# Language files

The pages' texts live in `en.json`, one flat key for each text. The Rust
source uses the keys, and `crates/skyward-ui/build.rs` turns the file into
the constants the pages draw.

The file is plain JSON with string values, which Crowdin imports and exports
without a custom format:

```json
{
  "install": "Install",
  "couldnt_mount": "Couldn't mount {base}"
}
```

Placeholders use the same `{name}` syntax as Rust's `format!`. A translation
keeps the placeholders and changes only the words around them.

To add a language, add a file named for its locale, such as `zh-CN.json`, with
the same keys. The current build carries English alone; adding a language
means teaching `build.rs` and `skyward-ui` to choose that file, which is the
next step when translations arrive.
