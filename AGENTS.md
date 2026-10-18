# AGENTS.md

Guidance for AI coding agents (and humans scripting one) working in this repo.
Read this first; it exists to stop you from breaking things that already work.

PetWeave is a Wayland-native desktop pet **framework** in Rust: a host
runtime (`crates/petweave`) that draws pets onto `wlr-layer-shell` surfaces,
and a shared contract crate (`crates/petweave-core`). Documentation is in
Chinese; **write user-facing docs and commit messages in the language already
used by the file you are editing** (docs/README in Chinese, code comments in
English).

---

## 1. Commands that actually work here

Treat the network as unavailable for normal builds: the current lockfile is
fully vendored. Downloading a **new** dependency needs network (tell the human
first). The default `~/.cargo` is partially read-only, so always use the
in-repo cargo home and `--offline`:

```bash
CARGO_HOME=$PWD/.cargo-home cargo build --offline
CARGO_HOME=$PWD/.cargo-home cargo test  --offline      # 158 tests: 98 host + 38 core + 21 settings + 1 GUI
CARGO_HOME=$PWD/.cargo-home cargo clippy --offline --all-targets
```

Without `CARGO_HOME=$PWD/.cargo-home`, `cargo build` fails with
`Read-only file system (os error 30)`. `.cargo-home/` holds a vendored cargo
registry (~1900 crate files, GUI deps included); do not delete it. To add a dependency you must first
make it resolvable offline — if it is not in `.cargo-home/registry`, say so
instead of silently changing the build to require network.

**Never** reformat the whole tree: `cargo fmt --all` produces a ~945-line diff
because the codebase is not rustfmt-clean. Match surrounding style by hand;
keep `cargo fmt` to the lines you already touched, if at all.

### Testing without a compositor

Most logic is deliberately testable without Wayland. Prefer adding a pure
function + unit test over anything that needs a live session:

```bash
CARGO_HOME=$PWD/.cargo-home cargo test --offline -p petweave-core    # config/placement/trait
CARGO_HOME=$PWD/.cargo-home cargo test --offline -p petweave --lib   # host logic
CARGO_HOME=$PWD/.cargo-home cargo test --offline -p petweave-settings # store/client/UI (headless)
```

The settings GUI is a separate crate (`crates/petweave-settings`, eframe/egui,
MSRV 1.95). Its dependencies are vendored in `.cargo-home` like the rest, so
the commands above still work offline; only adding a *new* crate needs network
first. The host (`petweave`) must stay free of egui/eframe/winit.

To verify rendering/loading without a compositor:

```bash
./target/debug/petweave --preview out.png -c your.toml   # no Wayland needed
./target/debug/petweave --preview out.png -c your.toml --pet-id bongo-main
./target/debug/petweave doctor                            # config + loads + permissions
```

Running the real host needs a Wayland session **and** a writable
`$XDG_RUNTIME_DIR` for the singleton PID file. If the default one is
read-only, point it elsewhere but keep the socket absolute, e.g.
`XDG_RUNTIME_DIR=$PWD/.tmp-run WAYLAND_DISPLAY=/run/user/1000/wayland-1`.
Run it under `timeout` so a hang cannot block you. Remember the host is a
singleton: a stale `petweave` process makes every later launch fail with
"another petweave instance is already running".

---

## 2. Architecture invariants (do not violate)

Full detail: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). The rules that
break silently if ignored:

- **All Wayland and pet state changes happen on the main thread** (calloop).
  Background threads (input reader, sysinfo sampler, config watcher, tray)
  only do IO and deliver messages through `calloop::channel`. Do not call
  Wayland proxies or mutate pets from a worker thread.
- **One `PetSurface` per pet, one shared `WaylandState`.** The connection,
  registry, `CompositorState`, `Shm` and `LayerShell` are process-wide;
  surfaces, SHM pools, buffer_scale, anchor/margins and input regions are
  per-pet. Do not reintroduce global per-pet state.
- **Pets are addressed by stable id, never by array index.** Config order is
  the stacking order, but ids (from `[[pets]].id`, generated as `{kind}-{n}`
  when absent) are the addressing key used by the tray and the future control
  channel.
- **Layer surfaces never report their position.** Position is *mirrored*:
  `PetSurface` keeps `anchor` + `margins`, and
  `petweave_core::placement` derives the rectangle. Any new movement code
  must update the mirror, not just call the protocol.
- **Idle must stay at ~0% CPU.** The loop blocks in `poll` until
  `Runtime::next_deadline()` or the physics deadline. Never add an
  unconditional redraw tick.
- **`petweave` (host) must never gain a GUI dependency** (egui/eframe/GTK/
  winit). The settings GUI is a separate crate by design. Verify with
  `cargo tree -p petweave | grep -Ei 'egui|eframe|gtk|winit'`.
- **Pet implementations never panic.** Loading/reload failures return
  `Result<_, String>` and are logged; one broken pet must not take down the
  host or the other pets.

## 3. Config v2 rules (easy to get wrong)

The config is a user contract; changes are near-irreversible once shipped.

- `[render]` is **global defaults**; each `[[pets]]` entry is a
  `PetInstance` whose placement fields are `Option` and inherit when unset.
  Resolve through `PetInstance::resolve(&RenderConfig)` /
  `Config::resolve_pets()` — never read `[render]` directly for a pet.
- Backwards compatibility is mandatory: a lone `[pet]` table must keep
  working (read as one pet with id `default`), `[[pets]]` wins if both exist,
  and `[render].physics` still selects `mode` when `mode` is empty. There are
  tests for all of this — keep them passing.
- Surface size precedence is **`[[pets]].width/height` → pet's own preferred
  size → `[render]`** (`petweave_core::pet::resolve_surface_size`); one given
  side derives the other from the artwork's aspect ratio. BongoCat's natural
  size comes from `cat_height`.
- `Config::validate()` is the **single source of truth** for validation.
  The GUI must call it rather than re-implementing rules. Bounds live as
  constants in `config.rs` (`MAX_PETS`, `MAX_SURFACE_SIZE`, …).
- Files that write config use `toml_edit` to preserve comments and key order,
  plus atomic write (tmp + rename), a `.bak` copy, and `0600`. The host is
  read-only: only the CLI/GUI write config (decision D10).
- When you change the config schema, update **all** of:
  `petweave.toml.example`, `docs/GUI_ROADMAP.md` §5, `docs/ARCHITECTURE.md`
  §4.5.1, `README.md`, and add validation + migration tests.

## 4. Pet abstraction (`crates/petweave-core/src/pet.rs`)

Implement `Pet` for anything that draws. Notes:

- Yield `true` from `on_event`/`tick` only when a redraw is needed.
- Implement `next_deadline()` if your pet is time-driven, or the host will
  sleep through your animation.
- The host calls `set_pet_id()` after loading so instances of the same
  kind/package stay distinct — built-in pets get this via
  `petweave_core::pet_id_setter!()`; if you add a field implementing the
  trait by hand, implement it too.
- Use `surface_size()` (default impl is correct) for size logic.
- Assets: decode through `runtime::assets::AssetCache` so multiple instances
  share `Arc<Frame>`s. Do not decode PNGs per instance.

## 5. Adding a built-in pet

1. `crates/petweave/src/runtime/builtin/<name>.rs`, implement `Pet`; add
   `petweave_core::pet_id_setter!();` next to `name()`.
2. Export in `builtin/mod.rs`, load it in `runtime/mod.rs::load_pet`.
3. Add the kind to `PET_KINDS` in `config.rs` — note the array length is part
   of the type (`[&str; 4]`), so bumping it is required or the build fails.
   If the new kind needs a package like sprite/lua, also update the
   `matches!(.., "sprite" | "lua")` rule in `Config::validate_pet`.
4. Prefer proving the framework through a **role package**
   (`packages/`, `docs/PACKAGES.md`) over a new built-in: built-ins are
   examples, packages are the product. Only add a built-in when the
   capability genuinely cannot be expressed declaratively/Lua.
5. Tests: load failure path, event→frame change, reload/preferred size.

## 6. Change checklist (finish every milestone this way)

The project's definition of done, from `CONTRIBUTING.md`:

- code + **tests** + docs (README / ROADMAP checkboxes) + real verification.
- Commit messages: Conventional Commits with a scope, e.g.
  `feat(M3.5 G0): multi-pet runtime and config v2`, `fix(drag): …`.
  Look at `git log` — the body explains what/why and how it was verified.
- Tick the right boxes in `docs/ROADMAP.md` (and `docs/GUI_ROADMAP.md` for
  GUI work; it is maintained incrementally with a progress board and ADRs).
- If you measured something (binary size, RSS, test count), record the number
  in the docs instead of an adjective.

## 7. Code in this repo that is not yours to clean up

- `Reference/` (gitignored) holds third-party sources used as reference
  (e.g. `wayland-bongocat`). Read it, never modify or vendor from it without
  checking its license.
- `.obsidian/`, `*.log`, `.scratch*`, `target/`, `.cargo-home/` are
  gitignored local state. Do not commit them.
- Pre-existing clippy warnings in `manifest.rs`, `render.rs`, `graphics/mod.rs`,
  `package.rs`, `demo.rs` are known; do not "fix" unrelated files in a
  feature commit (it makes review noise and risks behavior changes).
- There may be stashes, untracked docs, or uncommitted work from other
  people. Never `git stash pop`, `git checkout --`, or `git reset --hard`
  without an explicit instruction; if you must measure a baseline, use
  `git stash push --include-untracked` and restore it immediately, then
  verify `git status` matches what you saw before.

## 8. Where to look

| Question | File |
|---|---|
| How is it structured, what flows where | `docs/ARCHITECTURE.md` |
| What's done / planned (M0–M4) | `docs/ROADMAP.md` |
| GUI plan, config v2 spec, control protocol | `docs/GUI_ROADMAP.md` |
| Why these dependencies, platform constraints | `docs/TECH_STACK.md` |
| Writing a role package | `docs/PACKAGES.md` |
| Example config with every key documented | `petweave.toml.example` |
| Contribution / ecosystem rules | `CONTRIBUTING.md` |

When documentation and code disagree, **the code wins** — fix the doc in the
same commit and note the divergence (see `docs/GUI_ROADMAP.md` §13 for open
questions such as Q9 on size precedence).
