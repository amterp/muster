# Canvas panes

A canvas shows one Markdown, HTML, or image document in a Muster pane. Open one to show a report,
diagram, or preview beside the work that produced it. The pane stays open when its opener closes.

```sh
muster canvas open report.md --title "Test results"
muster canvas write revised.md --title "Test results"
muster canvas read --pane otter --json
muster canvas read --pane otter --output report.md
muster pane close --pane otter
```

`open` prints the new pane's callsign. It splits the calling pane in half to the right. With a
window, a command run outside a Muster pane uses the focused pane. Use `--left`, `--up`, `--down`,
or `--tab` to place it elsewhere. The keyboard stays where it was unless you pass `--focus`.
Use `--pane` to choose a pane to split. With both `--pane` and `--daemon`, the canvas goes beside that pane and runs on the
named daemon.

`open` and `write` take a file path. Omit it or pass `-` to read stdin. `--kind markdown`,
`--kind html`, or `--kind image` overrides the file extension. Input without a recognized extension
defaults to Markdown. Markdown and HTML use UTF-8. Image input can be PNG, JPEG, GIF, WebP, or SVG.
The document limit is 4 MiB. A title can be up to 256 UTF-8 bytes and contains no control characters.

`write` replaces the whole document and prints its new revision. Include `--title` to give the
replacement a title. `write` and `read` use the one open canvas that the calling pane opened.
Pass `--pane` with the canvas's callsign when you opened several or are calling from elsewhere.

`read` writes the original document to stdout. Use `--output FILE` to save any kind of document
to a local file. A successful export replaces the destination with a private file containing the
original bytes. For an image, use `--output` or redirect stdout to a file or pipe. `--json` prints the kind, title, and
revision, plus `image_type` for images. Combine `--output` and `--json` to save the bytes and
print the metadata.

When no window answers, or with `--no-window`, the commands ask the local daemon. From a plain
terminal, `open` puts the canvas in a new tab unless you name a pane to split. `--tab` opens a
new tab even from inside a pane. `--focus` and `--daemon` need a window. Without a window,
`write` and `read` resolve an explicit pane or the one canvas this pane opened on the local daemon.

Canvas commands require daemon protocol 1.17 or later. If a write exits with status 4, read the
canvas to check whether it arrived before sending another.
If a directory sync fails after the daemon replaces the document file, `write` exits 1 and
prints the daemon's durability message. The daemon rereads the file and publishes the revision
it finds. Read the canvas before sending another write.
