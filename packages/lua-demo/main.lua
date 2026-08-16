-- Lua demo pet: blinky squares with speech bubbles.
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
