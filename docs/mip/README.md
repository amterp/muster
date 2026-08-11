# Muster Improvement Proposals (MIPs)

MIPs record Muster's large decisions - what we chose, why, and what we rejected. One document spans the decision's
whole life: it starts as a proposal and matures into the permanent record.

The bar is deliberately high. The default home for rationale is the commit message: every commit here explains its
why, and most decisions never outgrow that. A MIP is for the rare decision that does - hard to reverse, shaping the
architecture or product in a way future readers will question, or where the roads not taken deserve a permanent
record. Two tests: if the why fits comfortably in a commit message, it is not a MIP; if you would want the full
case in front of you a year from now - alternatives, trade-offs, what would change the answer - before daring to
revisit it, it is. Expect a handful a year, not one per feature.

New MIPs copy [`template.md`](template.md). Numbers are sequential, files are `NNNN-short-slug.md`, and prose says
`MIP-1` (no padding). Statuses: **Draft** (being figured out, freely editable), **Accepted** (committed, not yet
built), **Implemented** (built; the document now describes reality), **Rejected** (kept, with a Rejection section
saying why the case did not win), **Superseded** (replaced by a later MIP). Once a MIP leaves Draft it freezes:
corrections get a superseding MIP, and the History section records the transitions.

## Index

| ID | Title | Kind | Status |
|---|---|---|---|
