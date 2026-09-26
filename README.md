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

// target_addr obtained via procmod-scan or manual inspection.
// SAFETY: the game has not started calling this function yet, and the hook
// lives until the mod shuts down after the game loop has stopped
let hook = unsafe {
    Hook::install(target_addr as *const u8, damage_detour as *const u8)
}?;

TRAMPOLINE.store(hook.trampoline() as *mut u8, Ordering::SeqCst);
// damage_detour is now called instead of the original function
```

## API

- **`Hook::install(target, detour)`** - Redirect `target` to `detour`. Returns a hook with a trampoline to the original.
- **`hook.trampoline()`** - Pointer to the original function's relocated entry point. Transmute it to the original signature to call it.
- **`hook.unhook()`** - Restore the original bytes and page protections, then free the trampoline. If removal fails, the hook stays installed and comes back inside the `UnhookError` so the removal can be retried.

Dropping a hook also removes it. If that removal fails, the hook stays installed and its trampoline is leaked, so a detour that calls it keeps working. To keep a hook for the rest of the process, pass it to `std::mem::forget`.

## How it works

1. Decode the whole instructions that the jump will overwrite, reading only as far as the target's memory is readable
2. Allocate a read-write trampoline within 2GB of the target
3. Relocate the stolen instructions into the trampoline, adjusting RIP-relative addressing, and append a jump back into the target
4. Make the trampoline read-execute
5. Overwrite the target's first bytes with a jump to the detour. Each affected page is made writable while keeping execute permission, then returned to its original protection
6. Synchronize instruction caches

Patching is transactional. If any step fails, the original bytes are put back and the page protections restored before the error is returned. Hooks installed by this crate are serialized against each other, and a hook that would overlap an existing one is rejected.

## Platform support

| Platform | Architecture | Status |
|----------|-------------|--------|
| Linux | x86_64 | Supported |
| Windows | x86_64 | Supported |
| macOS | x86_64 | Supported |

arm64 support is a future goal. The crate compiles on arm64 but exports no types.

## Safety

`Hook::install` is unsafe, and the caller's obligations cover the hook's whole life, including removal by `unhook` or drop:

- `target` must be the first instruction of a function, and its code must stay mapped while the hook exists
- The detour must have the same signature and calling convention as the target, and must stay valid while the hook exists
- No code may branch into the overwritten instructions other than to `target` itself
- While the hook is being installed or removed, no thread may be executing an instruction that starts within the first 14 bytes of `target`. procmod-hook does not suspend threads, so this is up to your own synchronization
- The trampoline may only be called while the hook exists. When the hook is removed, no thread may be inside the trampoline or about to call it
- No code outside this crate may modify the patched bytes, or change their page protection, while the hook is being installed or removed

## Upgrading from 1.x

See [CHANGELOG.md](CHANGELOG.md).

Part of the [procmod](https://github.com/procmod) ecosystem.
