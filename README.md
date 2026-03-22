<div align="center">

<img src="logo.svg" width="128" height="128" alt="procmod-hook">

# procmod-hook

[![crates.io](https://img.shields.io/crates/v/procmod-hook.svg)](https://crates.io/crates/procmod-hook)
[![test](https://github.com/procmod/hook/actions/workflows/test.yml/badge.svg)](https://github.com/procmod/hook/actions/workflows/test.yml)
[![license](https://img.shields.io/crates/l/procmod-hook.svg)](LICENSE)

Inline function hooking and detouring for x86_64.

</div>

## Example

Hook a game's damage calculation to make the player invincible:

```rust
use procmod_hook::Hook;
use std::sync::atomic::{AtomicPtr, Ordering};

static TRAMPOLINE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

extern "C" fn damage_detour(entity_id: u32, amount: f32) -> f32 {
    if entity_id == 1 {
        return 0.0; // player takes no damage
    }
    let original: extern "C" fn(u32, f32) -> f32 = unsafe {
        std::mem::transmute(TRAMPOLINE.load(Ordering::SeqCst))
    };
    original(entity_id, amount)
}

// target_addr obtained via procmod-scan or manual inspection
let hook = unsafe {
    Hook::install(target_addr as *const u8, damage_detour as *const u8)
}?;

TRAMPOLINE.store(hook.trampoline() as *mut u8, Ordering::SeqCst);
// damage_detour is now called instead of the original function
```

## API

- **`Hook::install(target, detour)`** - Redirect `target` to `detour`, returns a hook with a trampoline to the original.
- **`hook.trampoline()`** - Pointer to the original function's relocated entry point. Transmute to the original signature to call it.
- **`hook.unhook()`** - Remove the hook, restore original bytes, free the trampoline.

Hooks are automatically removed on drop.

## How it works

1. Decode instructions at the target function's entry point using [iced-x86](https://crates.io/crates/iced-x86)
2. Allocate executable memory (trampoline) within 2GB of the target
3. Relocate stolen instructions into the trampoline, adjusting RIP-relative addressing
4. Append a jump from the trampoline back to the target (continuing original execution)
5. Overwrite the target's first bytes with a jump to the detour

The detour runs instead of the original. It can call the original at any point through the trampoline.

## Platform support

| Platform | Architecture | Status |
|----------|-------------|--------|
| Linux | x86_64 | Supported |
| Windows | x86_64 | Supported |
| macOS | x86_64 | Supported |

arm64 support is a future goal. The crate compiles on arm64 but exports no types.

## Safety

Hook installation is inherently unsafe:

- No thread may be executing the target function's entry point during install/unhook
- The detour must have the same calling convention and signature as the target
- The hook must remain alive as long as the detour might call the trampoline

Part of the [procmod](https://github.com/procmod) ecosystem.
