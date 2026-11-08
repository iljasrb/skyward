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
keeps the placeholders and changes only the words around it.

To add a language, add a file named for its locale, such as `zh-CN.json`, with
the same keys. The current build carries English alone; adding a language
means teaching `build.rs` and `skyward-ui` to choose that file, which is the
next step when translations arrive.

## Keys

A key is its English text, lowercased, with apostrophes and placeholders
taken out and every other run of punctuation and spaces turned into one
underscore:

| Text | Key |
| --- | --- |
| `Install` | `install` |
| `A partition didn't verify.` | `a_partition_didnt_verify` |
| `A copy of the key: {shown}` | `a_copy_of_the_key` |

The constant the source names is the key in capitals, after `S_`:
`S_A_COPY_OF_THE_KEY`.

## Syncing upstream

Upstream's pages write every text three times, once per language, as
`["简体", "繁體", "English"]` arrays, `l.pick([...])` or `tr!(l, ...)`. This
fork keeps them in `en.json`, so a merge of upstream lands with texts in the
wrong shape.

`scripts/locales.py` closes the gap:

```sh
python3 scripts/locales.py check     # what is still written in three languages
python3 scripts/locales.py convert   # move it into en.json, name the keys
```

`check` runs in CI, so a merge cannot land with the texts unconverted. It
also names the lines a person still has to look at: a `Lang::Hant` in a test
whose expected text is Chinese, or an array the script cannot take apart.

`convert` is safe to run twice, and `--dry-run` says what it would do. It
keeps the English text alone; the other two go, since the pages speak
English.

`crates/skyward-ui/src/lang.rs` carries `Lang::pick` and `tr!` for exactly
this reason: they take the three texts and give back the English one, so a
merge builds as it lands, before `convert` has run. Nothing in the tree
should still call them once `check` passes.
