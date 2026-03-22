<div align="center">

<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="128" height="128">
  <rect width="256" height="256" rx="16" fill="#111"/>
  <line x1="36" y1="96" x2="96" y2="96" stroke="#f97316" stroke-width="8" stroke-linecap="round"/>
  <path d="M 96 96 L 96 148 Q 96 184 128 184 Q 160 184 160 148 L 160 96" stroke="#38bdf8" stroke-width="8" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
  <line x1="160" y1="96" x2="220" y2="96" stroke="#f97316" stroke-width="8" stroke-linecap="round"/>
  <path d="M 210 86 L 224 96 L 210 106" stroke="#f97316" stroke-width="6" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
  <circle cx="96" cy="96" r="6" fill="#a78bfa"/>
  <circle cx="160" cy="96" r="6" fill="#a78bfa"/>
</svg>

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
