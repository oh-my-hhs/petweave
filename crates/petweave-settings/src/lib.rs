//! PetWeave settings panel library.
//!
//! The binary (`src/main.rs`) is a thin eframe wrapper. Everything that can
//! be tested without a compositor lives here:
//!
//! - [`store`]: comment-preserving config read/write (atomic, `.bak`, 0600),
//! - [`client`]: control-channel client (`$XDG_RUNTIME_DIR/petweave/control.sock`),
//! - [`app`]: GUI state and eframe rendering,
//! - [`fonts`]: CJK font injection, [`i18n`]: user-facing strings.

pub mod app;
pub mod client;
pub mod fonts;
pub mod i18n;
pub mod store;
pub mod ui;
