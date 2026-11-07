# Tools in the recovery ramdisk

The recovery binary answers to the names install scripts use for the tools a
recovery gives them: `7za`, `zstd` and `unzip` are built in and linked beside
it in `/system/bin`, so no separate programs are carried for them. Their
licences are in `../licenses`.

`skyward-pack recovery --tools <folder>` still copies every regular file and
symbolic link in that folder into `/system/bin`, for a device branch that
needs another program there. The folder is empty here.
