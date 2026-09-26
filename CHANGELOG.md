# Changelog

## 2.0.0

This release makes hook removal sound, makes patching transactional, and fixes several ways installation could crash or corrupt the target.

### Breaking changes

- `Hook::unhook` is now safe and takes `self`. Removal obligations are part of `Hook::install`'s safety contract, which is what made removal on drop sound. On failure it returns an `UnhookError` holding the still-installed hook, which `into_hook` recovers.
- `Error::NotInstalled` is removed, since a hook can no longer be removed twice.
- `Error` is `#[non_exhaustive]`. New variants: `QueryFailed`, `FlushFailed`, `NotExecutable`, `PrologueUnreadable`, `AlreadyHooked`, and `PatchModified`. `ProtectFailed` now carries the page address and the operating-system error.

### Fixes

- The prologue is read only as far as memory is readable. A function ending just before an unmapped or inaccessible page no longer crashes installation.
- Enough of the prologue is decoded for any instruction the patch overlaps. Previously an instruction extending past the 16th byte failed, and 14-byte absolute patches could fail spuriously.
- Relocated code is checked against the trampoline's size before it is written.
- Each page's original protection is restored individually. Linux and macOS previously forced read-execute, and Windows applied the first page's protection to every page.
- Code pages keep execute permission while they are patched, so other code on the same page can keep running. Previously macOS removed it.
- Trampolines are written while read-write, then made read-execute. They are no longer left writable and executable.
- Instruction caches are synchronized after every code write: `FlushInstructionCache` on Windows, `membarrier` with core serialization on Linux, and `sys_icache_invalidate` on macOS.
- A failed protection change or cache flush rolls back to the original bytes instead of being ignored.
- Removing a hook whose bytes were changed by other code fails with `PatchModified` instead of overwriting them.
- Hooks are serialized against each other, and overlapping hooks are rejected with `AlreadyHooked`.
- `Hook` is `Send` and `Sync`, so it can be stored in a static.

### Migrating

```rust
// 1.x
unsafe { hook.unhook()? };

// 2.0
if let Err(error) = hook.unhook() {
    eprintln!("{error}");
    let hook = error.into_hook(); // still installed, retry later or keep it
    std::mem::forget(hook);
}
```

Review the safety comment on each `Hook::install` call against the new contract. It now also covers the moment the hook is removed or dropped.
