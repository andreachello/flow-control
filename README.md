# Flow control

Version control tracks files. Flow control tracks what your product does.

AI changes too many files at once for anyone to review by reading diffs. Flow control reads a repo as its user journeys instead: "a guest requests a booking", "a free user upgrades". Each journey is a flow, the ordered set of code that runs for it. When code changes, the flows that changed are what you review.

The `prototypes/` folder holds the seven browser prototypes from the design conversation. They run on a toy app. This crate is the first real build: a fast, deterministic lifter in Rust that reads TypeScript repos.

## What it does today

`flow lift` reads every TypeScript and JavaScript file with tree-sitter and writes a flow document to `flows/`, meant to be committed:

- `<flow>.flow.yaml` has one file per journey. It lists every step in call order, with the lines it owns, a content hash, and the state it reads and changes. Changing a function changes the hash in exactly the flows that run it, so `git diff flows/` shows which journeys a change touched.
- `_ledger.yaml` gives every line exactly one owner. Placed plus unplaced equals the total, and the slices print back to each file byte for byte. A statement that runs when a file loads, outside any function, is unplaced: the lifter will not guess where it belongs.
- `_slices.yaml` lists every slice, the flows that run it, and the code no flow reaches.
- `_index.yaml` holds the counts and a fingerprint. The same repo always lifts to the same bytes.

Journeys are found by convention: Next.js route handlers, pages (with their layouts, loading and error boundaries, and `generateMetadata`), metadata routes, server actions, Pages Router files and middleware. Anything else can be declared in `flow.toml`.

A call like `plugins[name]()` on a `const` object written in the code can only pick from that object, so the table and the functions it names join the journey. A table that can change (a spread, a computed key, a `let`, or a write elsewhere) joins too, and the call stays reported.

Anything the lifter cannot prove is reported, never guessed. That includes a call looked up by name on something that is not a fixed table, a name that is not declared anywhere, an event whose name is not a fixed string, and syntax the parser could not read.

## Every language

Flow reads every text file in a repo, in any language, including languages it has never heard of. There are three levels of understanding, and a file gets the best one available:

The universal reader needs no grammar. It finds what nearly all languages share: blocks at the left margin, brackets and indentation that keep them open, comments and decorators above them, methods one level inside a class, names followed by `(`. That gives one owner per line, merging by function, comparing without formatting, moves and renames, and journeys linked by name within a language. Routes are recognised from shapes most frameworks share (`@app.get("/x")`, `@GetMapping("/x")`, `#[get("/x")]`, `HandleFunc("/x", h)`, `app.post("/x", h)`, `path("x/", view)`), along with `main` functions and `if __name__ == "__main__"`. When a repo has no entry points at all, the functions nothing else calls become the starting points. Anything else can be declared in `flow.toml`.

A grammar reader follows real imports and types. TypeScript and JavaScript have one today, so their journeys are the most precise. Without a grammar, names are linked by matching: a name that exists once in the language links to it, and a name that exists twice links only if one is in the same folder. It can mislink when different things share a name, so treat those journeys as a good map, not proof.

Lockfiles, binaries, generated bundles and files over 1.5MB stay in version control but are not read.

## Commands

```
flow lift                  # write flows/
flow check                 # for CI: exit 1 if flows/ is stale or a line has no owner
flow show "POST /api/upgrade"
flow ledger                # files with unplaced lines
flow ledger lib/mailer.ts  # owner of every line
flow name                  # Claude proposes plain names into flow.names.json
flow view                  # open the journeys in the browser
flow status | save | log | history | explain | merge | resolve   # version control, below
```

Use `-C <repo>` to point at another repo, and `--out <dir>` to write somewhere other than `flows/`.

## Version control without diffs

Git never stored diffs; it stores a snapshot of every file at every commit. What is built around diffs is everything people see: review, merges, conflicts. Flow keeps git as a silent snapshot store, so GitHub and deploys keep working, and answers every question in journeys instead:

```
flow status                      # which journeys your unsaved edits change
flow save -m "Annual plans"      # a git commit whose message says which journeys changed
flow log                         # history as product changes
flow history "Guest asks to book a stay"
flow explain <commit>
flow merge <branch>...           # merge function by function, one branch or several
flow resolve "Resolve price" theirs
flow merge --abort
```

A merge takes each whole function from whichever side changed it. If both sides changed the same function differently, that is a collision: our version stays until someone decides, and `flow save` refuses until every collision is decided. Imports and other loose lines merge as a set. When each side changed a different step of the same journey, the text merges cleanly but the result is something neither side has seen, so the merge names those journeys. That is the $192 case from the prototype: annual pricing and a loyalty discount merge without a conflict and meet in the upgrade journey.

Merges also handle the cases git handles badly. A function that only one side reformatted never conflicts, because changes are compared by behaviour: spacing, line breaks and comments do not count. Two edits to different lines of one function, even neighbouring lines, are combined and listed so someone can check them; only the same line changed two ways is a collision. A function one side moved to another file keeps the other side's edits at its new place, and the change report says "moved" or "renamed" instead of one function removed and another added. Files that are not code are merged as text with git's own merge, and named.

A merge is as repeatable as git's: its result is a git tree that depends only on the commits, so two machines can compare tree ids. Your git settings, and files git ignores, have no say. A merges into B to the same tree as B into A, and `flow merge a b c` gives the same tree in any order. Branches that merged each other have several common ancestors, which are first merged into one virtual ancestor, as git does; anything that ancestor could not settle stays a collision. Several branches at once need no decisions, as with git's octopus merge, so one collision stops the merge before anything is written. Merge those one at a time.

Git writes the result, so binary files, file modes and links come through exactly, and a merge refuses to overwrite a file git does not track. Files in `flows/` are never merged: they are rebuilt from the merged code. In a git repository, `flow lift` reads exactly what a save would record: tracked files plus new files that are not ignored.

Not yet: running scenarios to check what a merged journey actually does, so a meet is still a warning rather than a verdict.

## Plain names

`flow name` asks Claude (through the `claude` command, so no API key is needed) to name every journey and step in plain words and to sort journeys into areas such as "Owner dashboard". The names are saved to `flow.names.json`, which is meant to be committed and reviewed. Each name stores the hash of the code it describes: when that code changes, the viewer marks the name as stale and `flow name` proposes a new one. To correct a name, edit the file and set `"by": "person"`; `flow name` never overwrites those. Names never change what a flow contains.

`flow view` opens every journey in the browser with those names, and falls back to names guessed from the code.

## flow.toml

```toml
ignore = ["scripts/"]

# A journey the conventions do not find.
[[entry]]
file = "lib/jobs.ts"
export = "nightlyDigest"
id = "job nightly digest"

# Give an unplaced statement an owner. Matched by the statement's first line.
[[assign]]
file = "lib/mailer.ts"
line = 'on("plan_changed", onPlanChanged);'
to = "onPlanChanged"

# Follow an event bus: register("evt", handler) plus emit("evt").
[[rules.event]]
register = "on"
emit = "emit"
```

## Known limits

Calls on values are followed by where the value came from: `new X()` or `: X` for a class in the repo leads into X, a value made by a package (a database client, a transaction handed to a callback) counts as that package, and list, text and date methods are built in. On Italiando 99% of method calls are placed this way; the rest are counted as unknown in `_index.yaml`. State footprints are one property deep on module-level data (`S.plans`), and mutation through methods (`list.push(x)`) shows up as a read only. Flows are marked `verified: false` until a scenario runs them.

## Next

1. Scenarios: one executable file per journey (starting state, action, expected end), run with tracing to record real steps and footprints, and to cross-check the static flows.
2. `flow diff <a> <b>`: run every journey on two commits and report what each one now does differently.
3. Collisions: run each change alone and merged to find where changes decide the same thing.
4. A local web UI built from the prototype screens: the journey player, before and after, and collisions.
5. Permanent flow and step IDs stored in the flow document, so renames do not break history.

`cargo test` runs the parser tests and the demo fixture in `fixtures/demo`, a TypeScript port of the prototype app.
