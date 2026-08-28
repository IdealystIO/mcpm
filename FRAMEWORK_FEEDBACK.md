# Idealyst feedback — from building the Control Center console

Notes from an agent building a real screen with this framework: a live
board, a `code_editor`-based capture composer with `#tag` highlighting
and Tab completion, and a `#[server]`-fn data path. Ordered by how much
time each cost, not by severity. Rev `d8226c7`.

## What worked unusually well

Worth saying first, because these are the parts that made the feature
possible in one sitting.

- **`codeblock::code_editor`'s decoration model.** Byte ranges plus
  field-wise layering meant `#tag` syntax highlighting was ~20 lines of
  app code and zero backend knowledge. "The primitive never parses
  anything" is exactly the right seam — I got to decide what a tag is,
  and the editor stayed a text editor. The module doc explaining *why*
  the decorated layer is the one in flow told me not to try to make it
  scroll internally, which I would otherwise have attempted.
- **`KeyEvent::selection_start` + the UTF-16 note.** Tab completion
  needs the real caret. Having it on the event (rather than only through
  an imperative handle) meant no caret tracking, and the doc comment
  saying offsets are UTF-16 code units while Rust strings are bytes
  prevented a real bug — I wrote the conversion because the docs told me
  to, and the emoji test passed first try.
- **`Tokenized::<Color>::resolve()`.** Decorations take literal colors,
  which normally means hardcoding hex and drifting from the palette.
  `resolve()` let the decoration colors come from the installed theme,
  reactively, so light/dark works with no per-mode code.
- **Compile-verified recipes.** `describe_recipe` beat every other form
  of documentation, because the answer is known to compile against the
  version I'm on. `input_with_submit` in particular saved me from the
  `on_key_down` footgun below.

## Friction

### 1. `ui!`'s `for` body is an `Fn` closure, and the error doesn't say so

The costliest one. This fails:

```rust
let tags = w.tags.clone();
ui! {
    view {
        if !tags.is_empty() { /* … */ }
        for tag in tags { TagChip(label = tag) }
    }
}
```

with `E0382: use of moved value` / `E0507: cannot move out of value, a
captured variable in an Fn closure`, both spans pointing at the whole
`ui!` block. Nothing names the for-each, and the fix — iterate
`0..count` and have an index-keyed child component re-read the data —
is not derivable from the message. I only found it by noticing that
existing code in the repo iterated indices everywhere.

Suggestions, cheapest first: a line in the `ui!` macro docs ("the `for`
body is an `Fn` closure, so it cannot consume captured values; iterate
indices and read in the child"), a note on the `__idealyst_for_each`
signature, or accepting an owned `IntoIterator` where the item can move.

### 2. `stylesheet!` rejects doc comments

```rust
stylesheet! {
    /// Sized for the longest label the button can produce.
    pub SubmitSlot<IdeaThemeRef> { … }
}
```

→ `error: expected identifier`. I've hit this in two separate sessions
and both times had to move the comment outside the macro, which
separates it from the thing it documents. Accepting (and ideally
emitting) `#[doc]` attributes on the sheet name would be a small win for
a macro that otherwise reads like ordinary Rust.

### 3. `*Ref`-typed props don't say how to spell their values

`describe_component(Button)` reports `size: Reactive<ButtonSizeRef>`.
That tells me the type but not the value vocabulary. I tried
`idea_ui::button_size::Sm` (no such module), then
`idea_ui::ButtonSize::Sm` — which is a *trait*, so `E0782: expected a
type, found a trait`, with rustc helpfully suggesting `<dyn
idea_ui::ButtonSize>::Sm` — before finding `idea_ui::size::Sm`.

The modifier ZSTs are a lovely design; the catalog just doesn't connect
the `Ref` type to its constructor namespace. A `values:` line on props
whose type ends in `Ref` (`values: idea_ui::size::{Sm, Md, Lg}`) would
close it. The same applies to `tone`, `variant`, `shape`,
`typography_kind`, which I only knew because existing code used them.

### 4. `on_key_down` as an inline prop is a silent no-op

The docs are emphatic and the recipe repeats the warning, so I didn't
get bitten — but only because I happened to read the recipe first. A
builder-only prop passed inline still compiles and silently does
nothing, which is the failure mode the framework elsewhere works hard to
avoid (cf. "an unregistered payload panics at realize, by design").
Since `text_input` *knows* `on_key_down` is one of its props, `ui!`
could reject it at compile time with "…is a builder method; chain
`.on_key_down(..)` after the call". Failing that, an `idealyst lint`
rule.

### 5. Focus vs. data-keyed `switch`

A `switch` keyed on polled data will rebuild any editable surface inside
it and steal focus mid-keystroke. That's inherent to structural
rebuilds, but it's a trap for exactly the app shape the framework
encourages (a signal-driven board polling a server fn), and the failure
only shows up when a *second* actor writes while you type — so it
survives local testing.

I had to hoist the composer's buffers onto a shared state struct and
deliberately exclude `rev` from the enclosing switch key. A paragraph in
the reactivity or components guide — "keep editable surfaces out of
switches keyed on asynchronously-arriving data; scope the switch to the
region that actually depends on it" — would have saved me discovering
that by reasoning about it. The `field_password_with_visibility_toggle`
recipe makes exactly this point for `secure`, so the principle is
already understood in the codebase; it just isn't stated where someone
laying out a screen will find it.

### 6. `register_scene_extensions` scaffolds with bounds no SDK satisfies

`idealyst new` generates:

```rust
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {}
```

but `codeblock::register` needs `H: StyleServices + TextOps`, so the
first SDK you add requires rewriting the signature, and what you get
first is a trait-bound error rather than a pointer. The `sdks` guide's
registration example also shows the `Host`-only bound, which is the
shape that *doesn't* compile; `crates/tools/docs-app` has the real one.
Either scaffold the wider bounds (commented, with the SDK list) or fix
the guide snippet.

### 7. A server-fn poll can fail forever with no trace

This is mostly my own bug — I wrote `if let Ok(snapshot) = result` and
dropped the `Err` — but it produced a console stuck on "connecting" with
nothing anywhere to explain it, and I burned a while on it. (Root cause
turned out to be a wedged host process, not the schema mismatch I first
suspected.)

The reason it was hard: `x-srv-schema` mismatch is a real, expected
failure with a specific 426 → `ServerError::IncompatibleVersion`
mapping, and it's invisible unless the app author chose to log it. That
particular error class is never something the app can handle — it always
means "your client and server were built from different sources" — so
the SDK logging it at `warn` unconditionally would cost nothing and turn
a mystery into a one-line diagnosis. Same argument for transport errors
on the very first call.

### 8. A props struct binds to exactly one component

Reusing `DrawerProps` for both `Drawer` and the `ModulePanel` I split
out of it fails with:

```
error[E0119]: conflicting implementations of trait `BuildElement` for type `DrawerProps`
```

with both spans on the `#[component]` attributes. The cause is
reasonable — the macro implements a trait keyed on the props type — but
the message names `BuildElement`, which app code never mentions, rather
than the rule ("a props struct may back only one component; give the
second its own"). Splitting one component into two is a common enough
refactor that the diagnostic is worth a note in the `#[component]` docs.

Related: the split was *forced* by `presence`, whose child closure is
`Fn` and so cannot capture the panel's ~15 non-`Copy` locals — the same
underlying constraint as friction 1. It would be worth saying once, in
the components guide, that `Fn`-closure child slots (`ui!`'s `for`,
`presence`, `switch`) all push data behind props or index-keyed
re-reads, since discovering it per-site costs the same time each time.

## Small things

- `Element` not being `Clone` is right, but the resulting error (`no
  method named clone`) doesn't hint that branch content belongs inside
  the `switch` closure.
- `Signal::update` taking `FnOnce(&T) -> T` rather than `&mut T` is
  unusual enough to be worth a line in the reactivity guide's signal
  section; I reached for `|v| *v += 1` first.
- `presence`'s docs are unusually good — the "NOT `when`" warning and
  the "reach for `presence` whenever a panel should fade or slide"
  framing meant the drawer animation was right first try. Worth noting
  what the exit path needs though: the child has to keep rendering
  while `present` is already false, so any target the child reads must
  outlive the close. A line about holding the last target would save
  the reader a blank-panel-sliding-out bug.
- `mcp__idealyst__screenshot`'s `width`/`height` are documented as
  honored by the replay path, but with a live client attached the
  arguments are silently ignored and you get the client's viewport. Both
  behaviours are defensible; the tool result could say which path it
  took.
