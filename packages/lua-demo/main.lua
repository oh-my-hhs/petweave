-- Lua demo pet: blinky squares with speech bubbles.
--
-- The script owns what is shown: the sprite is a passive image (`OWN_KEYS`)
-- and the animation is switched here. `pet.size(w, h)` sets the exact surface
-- size, `pet.w()` / `pet.h()` read it back.
OWN_KEYS = true

function init()
    pet.speak("hi! press keys")
end

function on_key(code, pressed)
    if pressed then
        pet.play("flash")
        pet.speak("key " .. code)
    end
end

function on_system(cpu, mem)
    if cpu > 90 then
        pet.speak("cpu is hot!")
    end
end
