# Earlier versions of your files
also: version versions snapshot snapshots history restore previous older backup time machine

Your disk can keep snapshots: a picture of every file as it was at one moment. A snapshot takes almost no space when it is made, and files in it never change.

Snapshots are kept from the shell window today:

- "snap create <name>" makes one now.
- "snap list" lists them, each with its number.
- "snap ls <number>" lists the files in one, and "snap cat <number> <file>" shows an old copy of a file.
- "snap drop <number>" removes one.

This works on the computer's own disk, not on a USB stick.
