# iroh-blobs 0.103.0, vendored

Upstream source, unmodified except for the change described here. Wired in from
the workspace root with:

```toml
[patch.crates-io]
iroh-blobs = { path = "third_party/iroh-blobs" }
```

## Why

`ImportMode::TryReference` lets a store reference a file where it already is
instead of copying it — which is the whole reason an Android send can cost no
disk. Referencing stores the *path*, and the store reopens that path whenever
it needs the bytes.

Android does not hand an app a path. It hands over a `content://` URI, and the
only filesystem name for the file it grants is `/proc/self/fd/<n>` — a magic
link onto the descriptor the app is holding. Opening that link is a fresh path
walk, it lands in MediaProvider's FUSE daemon, and the daemon re-checks
permission against the app's uid. The per-URI grant covers the descriptor that
was handed over, not a re-open by name, so unless the app holds
`MANAGE_EXTERNAL_STORAGE` the reopen fails with `EACCES` — measured on Pixel 7 /
Android 17 for every provider tried, including MediaProvider's own.

The descriptor stays perfectly readable throughout. Only the name is closed to
us. So the store needs to be able to read an external path through a handle
rather than by opening it.

## The change

A new module, `src/store/fs/preopened.rs`: a process-wide registry mapping a
path to a live `File`. `preopened::open(path)` returns a duplicate of the
registered handle if there is one, and falls back to `File::open` otherwise.
Reads in this store are positional (`read_at` / `read_exact_at`), so duplicates
sharing a file offset is not a problem.

Three call sites then go through it instead of opening the path directly:

- `src/store/fs.rs`, where a complete entry with `DataLocation::External` is
  materialised right after import. The import's own handle is deliberately
  dropped just above it and the path reopened moments later.
- `src/store/fs/bao_file.rs`, `BaoFileStorage::open`, which loads an entry back
  from the database — the path taken when the blob is *served*, and the one
  that decides whether a transfer actually works. Missing this one still
  imports fine and then fails the moment bytes are asked for, as
  `poisoned storage`.
- `src/store/fs/import.rs`, `import_path_impl`. A registered path cannot answer
  `is_file()`, `metadata()` or `std::fs::read` either, so when a handle is
  registered, existence, size and (for an inlined blob) the bytes all come from
  the handle.

Nothing is persisted: a descriptor only means anything to the process holding
it, so a store reopened later falls back to opening the path and fails honestly
rather than reading whatever that name points at by then.

## Carrying it to a newer release

1. Replace this directory with the new crate source, keeping `PATCH.md`.
2. Copy `src/store/fs/preopened.rs` back in and re-add `pub mod preopened;` to
   the module list in `src/store/fs.rs`.
3. Re-apply the three call sites above. All are small and easy to find:
   search for `DataLocation::External` — every match that opens a path needs
   the hook — and for `ImportMode::TryReference` in `import.rs`.
4. `cargo test -p iroh-blobs preopened` covers the registry itself. The
   end-to-end behaviour is covered by
   `import_reads_a_descriptor_whose_path_cannot_be_reopened` and
   `a_small_unreopenable_descriptor_is_inlined_from_its_descriptor` in
   `crates/core/src/blobs/util.rs`, which stage the refusal with `chmod 000`
   and so run on any unix (they skip themselves where the mode is ignored).

Worth upstreaming — the hook is small and the constraint is not specific to
Wisp.
