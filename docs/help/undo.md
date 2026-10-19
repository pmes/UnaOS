# Undo in Quarry
app: quarry
also: undo redo mistake take back ctrl-z revert

Quarry remembers what you did, so a mistake is one key away from being undone.

- Ctrl-Z undoes the last change: a move or rename goes back, a file sent to the Trash comes back, a new empty folder is removed, a copy goes to the Trash.
- Ctrl-Shift-Z does it again.
- The Edit menu names the step it will undo, for example "Undo Rename".
- The notice that appears when you delete a file has an Undo button.

Quarry remembers the last 64 steps until you log out. If something changed since, for example the file was moved again by another program, Quarry does not guess: it tells you why it cannot undo that step.
