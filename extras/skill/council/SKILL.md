---
name: council
description: Use when taking part in, directing, or convening a council - several agents and the human discussing, reviewing or deciding something together over `muster msg` - or when you are woken by a message from a group you were added to. Not for handing one agent a brief; `muster msg post --to` alone does that.
---

# Council

A council is a `muster msg` group whose members work one question together: a review, a
design, a decision. Each message is a turn someone pays for, in money and in the context every
member carries, so a council is only as good as its signal per message. `muster msg --help` has
the verbs; this is how to take part.

**The council hears only what you post with `muster msg post`.** A reply written in your own
turn reaches nobody, however complete it is: when you are woken and have something to say, the
turn ends with a post.

## Taking part

**Post through stdin with a quoted heredoc**, so the shell leaves backticks and `$` in your
text alone rather than running them:

```sh
muster msg post --group <group> - <<'EOF'
concern: `save()` rewrites the whole file, so two writers lose an update.
EOF
```

**Stay until you are dismissed.** Never run `muster msg leave` on your own. Staying costs
nothing: you are woken when a message is for you, and not otherwise. Leaving strands whoever
was about to address you, and in a directed council the policy refuses it anyway. When you
have nothing to add, end your turn.

**Do your part when you are woken**, with what you have. In a directed council another member's
post does not wake you, so ending your turn to wait for one waits forever; if your part needs
something you lack, say what to the director.

**Pass the turn by ending it.** Never post "standing by", "passing to X" or "done for now";
never run `muster msg wait` or poll `read` in the foreground to wait for others. You will be
woken, with a line saying how to read what came.

**Read before you post.** A post is refused while you have unread messages in its group, so
run the `muster msg read` the wake names, then decide whether your post still needs saying.

**Be terse.** Lead with your point. Aim under 150 words unless you are presenting analysis or
code. No compliments, thanks, summaries of what others said (they can read it), or offers to
help later - you will not be around later. Say "concern: X" or "decision: X, because Y", and
flag it when the discussion is circling.

**Address on purpose.** `--to NAME` wakes only NAME; an unaddressed post wakes whom the group's
policy rings for you. In a directed council a member addresses the director, or `@human` for
something only a person can decide; the policy refuses anything else, and the refusal says
whom you may address.

## Directing

In a directed council you are named for the group, `director-review` in `review`. Your
unaddressed post wakes every member but the human, and members' posts wake only you. So a member
learns of another's answer only when you wake it, and the human only when you address `@human`.

- Give each member a part: address them by name with what to do and what to report.
- Members can address only you and `@human`; a post from one member to another is refused. So
  never brief a member to post to another: ask for one post to you, and relay it.
- When one member's part needs another's answer, ask the first alone; when it answers, address
  the second with the number of the message to read.
- Decide. When members disagree, weigh it and post the decision with its reason; do not
  canvass again.
- Keep it moving: when a thread circles, say so and close it.
- Dismiss a member who is finished: `muster msg group remove <group> <name>`. Post anything it
  should know first; it can still read what you posted before removing it.
- Address `@human` only for what needs a person, in one message that says exactly what you
  need. When the council's question is answered, post the outcome to `@human` and end.

## Convening

Presets sit beside this file. `directed.toml` is a director with members who answer to it,
and either the director or the human may add members. `roundtable.toml` lets everyone address
and wake everyone, and only the human changes who is in it, so only the human can convene one.

`directed.toml` names its director `director-{group}`, and `group new` fills in the group's
name, so the director of `review` is `director-review`. A name belongs to one live session on a
machine, so each council needs its own director, and councils can run side by side.

Whoever makes the group is its first member, and under `directed.toml` only the director and
`@human` may add the rest, so `group new` refuses anyone else. Convene from the human's own
shell, where you are `@human`, and brief a director:

```sh
muster msg group new review --policy <this skill's directory>/directed.toml
muster msg group add review p2w3r07bsd p3w3r07bsd   # members: panes or participants' names
muster msg post --to p1w3r07bsd --file brief.md     # the director's brief
```

The director's brief tells it to run `muster msg join --name director-review --group review`
first: the director has to join from its own session, so that it is the one woken. A member
added by its pane needs no name. One that names itself with `muster msg join --name <name>`
keeps its place in the group; name it for the group too, `builder-review`, since another council
may already have a live `builder`.

An agent convening a council it will direct itself names itself first, then makes and fills
the group, and needs no brief: `muster msg join --name director-review`, then the first two
lines above. If that join is refused, `group new` is refused too, since you would hold no name
the policy lets add members.

A director that starts its own members, rather than convening agents the person started, makes
each with `muster pane new --helper`, so that it is told when one is blocked or stops without
answering, and the person is not.

`muster msg pause review` holds every wake while you catch up; `muster msg resume review` wakes
each member once for what it missed.
