# UX implementation guidelines

Standing rules for anyone (human or agent) implementing UI in this repo.
Read this before writing or reviewing screen code; it also serves as the
checklist for UX audits.

## This is a breathing document

When a UX issue is reported that is *general* — something that could
recur in other screens, existing or future (e.g. "spacing between
elements is inconsistent") — add it here as a new rule so the next agent
learns it and audits run against up-to-date criteria.

Keep it general. Do **not** add one-off, screen-specific notes ("in the
schedule cell picker don't use font size 12") — fix those in place. A
rule belongs here only if you can state it without naming a specific
screen.

## Rules

### 1. Respect idealyst component paradigms

Components are declared with the `#[component]` macro and named in
PascalCase:

```rust
#[component]
pub fn CrewSummaryCard(props: &CrewSummaryCardProps) -> Element {
    ...
}
```

A bare snake_case function returning `Element` "works", but it is not a
component to the framework and it is not clean. Any reusable piece of UI
gets the macro and a PascalCase name. (Small private helpers that merely
assemble children for one call site are fine as plain functions, but the
moment something is a *component* — has props, is mounted as a unit —
it follows the paradigm.)

Likewise, theme tokens in a `stylesheet!` are typed accessors on the
block binding — `t.spacing.sm()`, `t.color.border()` — never
`Tokenized::token("spacing-sm", fallback)` strings. The string form is
only for sheets with no vocabulary (`<()>`), app-defined token names,
or token names known only at runtime. A misspelled string compiles and
silently renders its fallback forever; a misspelled accessor fails the
build. Outside a stylesheet, `idea_ui::tokens()` gives the same typed
namespace.

### 2. Popovers stay tight

Popovers, flyouts, and menus with generous padding look bad. Keep their
internal padding minimal — a popover is a compact surface, not a card.
When in doubt, err on the tight side and let the content define the
size.

### 3. Padding goes *inside* the scroll view

Never wrap a scroll view in outer padding to space its content. That
insets the scrollbar off the container edge and clips content early —
both bad UX. Put the padding on the scroll view's content (or inside the
scrolling components themselves).

The inner placement matters especially with dividers: a divider should
span the full scrollable width, so padding belongs on the rows around
it, not on a wrapper above it — judge by context, but the scrollbar
always hugs the container edge and content never clips against an outer
pad.

### 4. idea-ui and the app theme beat the mockup

When implementing a Claude Design (or any external mockup): the existing
theme and idea-ui components take priority over pixel fidelity. If the
design shows a control that doesn't match an idea-ui component, the most
likely explanation is that the *design* strayed from the design system —
not that we need a bespoke component. Take layout and structure from the
design; take components, tokens, and brand styling from idea-ui and the
app theme.

### 5. Tables paginate and keep their shell

Data-heavy tables page at 20 rows with a summary line (and ghost
chevrons when there is only one page) — never render an unbounded
table. An empty table shows a blank-state row *inside* the table shell
(`widgets::blank_table_row`), never a lone sentence where the table
used to be.

### 6. One toolbar grammar

List screens share one layout: search field → Filter button → active
filter chips, left-aligned; date navigation is the shared DateJumper in
the header's right slot. No redundant date-range captions under the
toolbar. Filter state mirrors to the URL as `?k=v`.

### 7. Section hints are tooltips

Explanatory copy for a section header is a `?` icon with a Tooltip —
never a subtitle line under the header.

### 8. Forms show their coordinates

Every Select gets a placeholder, and an entry modal always shows the
coordinates it is scoped to (project, date, shift, person…). Never
validate a pick the form hides from the user.

### 9. Multi-step tasks are screens; detail views are drawers

Never stack a dialog inside a modal (nested modals are inert anyway).
A multi-step flow gets its own screen or a wizard surface; a non-modal
detail view is a right drawer, not another modal.

### 10. Refresh never blanks the body

Mutations reconcile in place — the settled-phase + reload-frame pattern
(`phase.settled()` + `widgets::reload_frame`), not a remount that
flashes the whole section on every save.

### 11. Spacing comes from theme tokens

Gaps and padding use the theme's spacing tokens, not magic pixel
values. If two sibling screens space the same construct differently,
one of them is wrong — match the established one.

### 12. Interactive means it looks interactive

Anything clickable gets `Cursor::Pointer` and a hover state. Custom
rows, icon buttons, and checkbox sheets are the usual omissions.

### 13. Don't nest cards

A card inside a card is almost always bad design. When a card needs
internal grouping, use spacing, dividers, or a quiet background band —
not another bordered/elevated surface. One card boundary per concept.

### 14. Modals close with the X

A modal already has a close affordance — the `X` icon button in its
header. Don't add a "Close" button to the footer alongside it. The
footer is for actions that *do* something: the primary verb (Save,
File, Approve) and, where a partial edit needs abandoning explicitly,
a "Cancel" next to it.

Use common sense about which one a surface needs:

- **Read-only / informational modal** — X only, no footer buttons.
- **Form or destructive confirm** — primary action, plus "Cancel"
  when dismissing means discarding work the user typed.
- **"Close" as the footer label** — only when it is genuinely the sole
  meaningful action and the flow reads better with a big target than a
  corner icon (a completion/receipt step, say). Never in addition to a
  Cancel.

### 15. A one-entry row menu is a clickable row

If a table's kebab holds a single action — and it almost always turns
out to be "Open" / "View" / "Edit" — delete the menu and the More
column and put the action on the row itself via
`TableRow(on_row_click = ...)`. Two clicks and a whole column for one
verb is a worse target and a worse table. `on_row_click` already gives
the pointer cursor and the hover highlight, so rule 12 comes free.

A kebab earns its place from the *second* entry: destructive or
secondary acts that must not fire on a stray row click (Archive,
Duplicate, Remove). When one is present, the row click still opens the
primary thing and the menu carries the rest.

### 16. Say it once

A tag, a button label and a paragraph explaining both is the same fact
three times, and the paragraph is the one that pushes the actual
content below the fold. Prefer the compact carrier:

- The **status** is the tag, not a sentence next to it.
- What an action does is the **button's own label** ("Acknowledge &
  release"), not a banner above it.
- A caveat that only applies while an action is *available* belongs in
  the button's disabled state or its confirm step, not standing text.

Reserve alerts and banners for what the reader cannot infer from what
is already on screen — an error, or a consequence the controls don't
name.

### 17. Labels name the field, not the developer's abstraction

A detail line labelled "Where" holding a shift name, or "Location"
holding a heading, is the code's internal grouping leaking onto the
screen. Label each fact with what it actually is — "Shift", "Heading" —
and split a packed line into separate labelled lines when the surface
has the room. A table cell may still combine them (`Day · Sump 3`) to
save a column; a detail panel has no such excuse.

Identifiers ride the thing they identify rather than taking a labelled
line of their own: an employee number is `Nicholas Mercier #2`, not a
"Employee number" fact underneath.

### 18. Chrome controls hold their width

A control whose label changes as the user drives it — a date or range
navigator, a count, a filter chip — must be given a fixed width, not a
floor. Otherwise every press resizes it and shuffles everything sitting
beside it in the toolbar or header, which reads as the whole bar
twitching. Size the box for the *longest* string the control can
produce (spell out the worst case: a range that straddles New Year
carries both years) and let short labels sit centred in the slack.

A floor (`min_width`) is only enough where the label barely varies at
all — a single day, a fixed set of words.

### 19. The app doesn't explain itself on screen

A checkin banner read:

> **Unresolved incident on site**
> It stays here until somebody resolves it in the app.

The first line is the whole message. The second is the *implementation*
narrated to the reader: it explains why the banner behaves the way it
does, to somebody who cannot act on that behaviour and did not ask.

Every line on a screen has to change what the reader does next. A line
that only makes the app's own logic legible — how state clears, why a
list is empty, what the code will do later, why a control sits where it
does — is a code comment that escaped into the UI. Write it as a code
comment, where it is genuinely useful, and delete it from the screen.

The tells, all of them second lines under something that already said
it:

- **Mechanism.** "…until somebody resolves it in the app", "this
  updates when the shift is submitted", "changes save automatically".
- **Empty-state narration.** "Nothing here yet — rows will appear once
  reports are filed." The blank row says "No rows" (rule 5) and stops.
- **Justifying a control.** "We ask for your PIN each time so
  corrections stay attributable." Ask for the PIN.
- **Restating the heading in a softer voice.** A section called
  Attendance overrides followed by "Here you can correct attendance."

Two things this does *not* forbid. An instruction the reader must
follow to use the thing ("Tap a name to correct their check-in") is
content, not narration — it tells them what to do, not how the app
works. And a genuine explanation of a domain rule still belongs on
screen, in the carrier rule 7 gives it: a `?` tooltip, or the disabled
state of the control it constrains — never as standing prose.

Closely related to rule 16: 16 is about saying one fact once, this one
is about facts that were never the reader's to hold.

### 20. A list row carries the item, not its properties

A row in a list is a *handle* on something, not a rendering of it. Give
it the one thing the reader scans for — the idea, the name, the
sentence — plus what they filter by (tags, status). Everything else
about the item is a **property**, and properties open in a drawer
(rule 9).

The tell is a list where each entry has grown a stack: an id line, the
content, a note, a provenance line, a footer of author and timestamp.
Six facts per row, five of them unscannable, and the list stops being a
list — you cannot see three entries at once, so the thing it exists to
give you (comparison across items) is gone.

What stays on the row:

- The item itself, in the author's words, unabbreviated.
- Its filing labels — tags, a status chip *where the surrounding
  section does not already say it*.

What moves to the drawer:

- Identifiers. Nobody scans for `want_9e0fecdd`; it rides the drawer
  header (rule 17).
- Provenance — author, captured-at, revision.
- Any per-relationship annotation: the *reason* a link exists is a
  property of the link, and a row has no room to argue.
- Free prose the reader did not ask for at scan time.

Corollary: two lists pointing at the same kind of thing open the **same**
drawer. One detail surface per entity, reached from wherever the entity
appears — not a bespoke expansion per list.

### 21. A banner is a current condition, not a past event

An alert, banner, or callout asserts *this is true right now*. If
nothing can clear it, it is not an alert — it is a log entry that
someone pinned to the top of the screen, and after the first read it is
pure noise the reader learns to scroll past.

Before adding one, name the thing that takes it down. If the answer is
"nothing" or "the whole record eventually disappears", it does not
belong in a banner:

- **A failure that already happened** — a rejected call, a denied
  claim, a retry that later succeeded — is history. It goes in the
  event feed and on the detail surface for the thing it happened to.
- **A condition that still holds** — a stage still locked, a queue
  still stalled, credentials still missing — is a banner, and it must
  disappear on its own the moment the condition ends.

The same fact often has both halves: the *attempt* is history, the
*blockage* is current. Show the blockage where the blocked thing is
(the locked lane says it is locked), and let the attempt live in the
ledger. Deriving a standing banner from "the most recent event of kind
X" is the anti-pattern — a rule with no clearing condition.

### 22. Text in a row must be told it may shrink

A `text` node in a flex row takes its **intrinsic, unwrapped** width. It
does not wrap to fit its container by default and it does not yield to
its siblings — so one long name in a card's title row pushes itself and
everything after it straight through the card's edge, over whatever
border was there.

`flex_shrink` alone does not fix it: there is nothing to shrink against
until the item is allowed below its intrinsic size. The pattern is a
slot per side:

- The **flexible** side (the title, the name, the sentence) gets
  `min_width: 0` **and** `flex_shrink: 1.0`. `min_width: 0` is the
  load-bearing half.
- The **fixed** side (a badge, a count, a chevron) gets
  `flex_shrink: 0.0`. A status pill squeezed to three letters is worse
  than a title on two lines.
- The **container** gets `overflow: Hidden` as the backstop, for content
  with no break opportunity in it at all — an id, a slug, a URL — which
  cannot wrap however much it is allowed to shrink.

The tell is a screen that looks right with your test data and breaks on
real names. Any row that pairs caller-supplied text with chrome needs
this; narrow surfaces (a sidebar, a drawer, a table cell) hit it first.
