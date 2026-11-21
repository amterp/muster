# Antigravity adapter

Muster reads the Antigravity CLI's screen from the `agy` detection manifest. The native plugin in
this directory reports turn boundaries and the conversation ID to the daemon that owns the pane.
The status-line wrapper reports state, the conversation title, model, effort and context usage.
Both use Python 3 on the machine running agy.

Install this directory with agy's native plugin command and start a new agy process to load it.

    agy plugin validate --strict /path/to/extras/agy
    agy plugin install /path/to/extras/agy

Set `statusLine` through your configuration manager or in agy's settings. For a one-session test,
put the object below in a separate JSON file and pass that file with `agy --settings`.

```json
{
  "statusLine": {
    "command": "/path/to/extras/agy/statusline.sh",
    "stack_with_default": true
  }
}
```

To wrap an enabled custom status line, pass its entire command as one shell argument. Keep its
existing padding and `stack_with_default` setting. Its command gets exactly the same JSON on
stdin and its output is the line agy displays.

For example, a settings manager that currently sets `command` to `~/bin/my-line --compact` can set
it to `/path/to/extras/agy/statusline.sh '~/bin/my-line --compact'`. Preserve `enabled: false` for
a disabled line. If you enable reporting later, omit the old command to keep that line disabled
and set `stack_with_default: true` so agy still draws its built-in status line.

The wrapper keeps daemon I/O off the status command's output path and suppresses repeated samples.
Context and title samples count once a hook identifies their conversation. On an explicit resume,
the next invocation identifies the conversation to the adapter. After a fully idle Stop, the
status line reports facts until the next invocation. When Escape or a background task leaves a
turn without a fully idle Stop, an idle sample can still report the turn's end. The wrapper
uses agy's shell command semantics, and reporting is quiet outside a foreground Muster pane.

[Antigravity plugin reference](https://antigravity.google/docs/plugins?tab=cli) ·
[Antigravity status-line settings](https://antigravity.google/docs/cli/statusline/)
