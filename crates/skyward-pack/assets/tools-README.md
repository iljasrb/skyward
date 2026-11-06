# Tools in the recovery ramdisk

The recovery image puts these programs in `/system/bin`, so a third-party
ROM's install script can use them the way it expects a recovery to. They are
not built here; their licences are in `../licenses`.

| program | version | source | SHA-256 |
|---|---|---|---|
| `7za` | 7-Zip 24.09 (arm64) | <https://github.com/user-attachments/files/34408854/tools.zip> | `9fcf394605239be338075856f40277397cf8c5f497226ad45764ca9013c703d9` |
| `zstd` | zstd 1.5.2 (Android arm64) | <https://github.com/user-attachments/files/34408854/tools.zip> | `b2b5618b51c0a9c5913b7d67a6fd2f50edc57293d8eb6a6c270cf994f9601720` |

Both are statically linked aarch64 programs: `7za` is a Linux static build and
`zstd` is an Android NDK build. They are the tools the reporter of
<https://github.com/kirklin/skyward/issues/20> supplied for unpacking a
`super.img.zst` inside a third-party ROM zip.

`skyward-pack recovery --tools crates/skyward-pack/assets/tools` copies every
regular file and symbolic link in that folder into `/system/bin`.
