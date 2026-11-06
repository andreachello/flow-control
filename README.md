# Flow control

Version control tracks files. Flow control tracks what your product does.

AI changes too many files at once for anyone to review by reading diffs. Flow control reads a repo as its user journeys instead: "a guest requests a booking", "a free user upgrades". Each journey is a flow, the ordered set of code that runs for it. When code changes, the flows that changed are what you review.

The `prototypes/` folder holds the seven browser prototypes from the design conversation. They run on a toy app. This crate is the first real build: a fast, deterministic lifter in Rust that reads TypeScript repos.

## Get started

Flow works inside any git repo. Install it, then connect the project once:

```bash
cargo install --git https://github.com/andreachello/flow-control --locked

cd your-project
flow lift                  # read the code and write flows/
git add flows
git commit -m "Add flow"
flow hook install          # connect Claude Code on this machine
flow name                  # optional: plain names for journeys and steps
```

Install flow before running `flow hook install`, because the hook stores the path to it. Add `--force` when updating an older copy. Then start a new Claude Code session in the repo. For Codex, `flow hook install` prints what to add by hand.

Next.js journeys are found automatically. In anything else, declare them in [`flow.toml`](#flowtoml).

Then work with your agent as usual:

1. Ask for a change. When the turn ends, flow shows which journeys changed and flags anything you did not ask for.
2. Answer any question it raises with `flow decide "<journey>" yes`, or `flow decide "<journey>" no "what it should do"`.
3. Run `flow diff` to see the change step by step in the browser.
4. Run `flow save -m "Annual plans"`, then `git push`.

`flow save` is a normal git commit of everything in the folder, including `flows/` and `decisions.md`. Branches, pushes and deploys stay plain git. If you use `git commit` yourself, run `flow lift` first, or `flow check` will fail.

Add these lines to `CLAUDE.md`, or to `AGENTS.md` for Codex, along with the `decisions.md` line `flow hook install` prints there:

```markdown
- Commit with `flow save -m "message"`, not `git commit`.
- Do not run `flow decide` or edit decisions.md unless I ask. Those answers are mine.
```

Nothing in flow stops an agent from answering its own question, so the second line matters.

## What it does today

`flow lift` reads every TypeScript and JavaScript file with tree-sitter and writes a flow document to `flows/`, meant to be committed:

- `<flow>.flow.yaml` has one file per journey. It lists every step in call order, with the lines it owns, a content hash, and the state it reads and changes. Changing a function changes the hash in exactly the flows that run it, so `git diff flows/` shows which journeys a change touched.
- `_ledger.yaml` gives every line exactly one owner. Placed plus unplaced equals the total, and the slices print back to each file byte for byte. A statement that runs when a file loads, outside any function, is unplaced: the lifter will not guess where it belongs.
- `_slices.yaml` lists every slice, the flows that run it, and the code no flow reaches.
- `_index.yaml` holds the counts and a fingerprint. The same repo always lifts to the same bytes.

Every journey and step also has a permanent `uid`, so a rename continues a history instead of ending one. See [Permanent ids](#permanent-ids).

Journeys are found by convention: Next.js route handlers, pages (with their layouts, loading and error boundaries, and `generateMetadata`), metadata routes, server actions, Pages Router files and middleware. Anything else can be declared in `flow.toml`.

A call like `plugins[name]()` on a `const` object written in the code can only pick from that object, so the table and the functions it names join the journey. A table that can change (a spread, a computed key, a `let`, or a write elsewhere) joins too, and the call stays reported.

Anything the lifter cannot prove is reported, never guessed. That includes a call looked up by name on something that is not a fixed table, a name that is not declared anywhere, an event whose name is not a fixed string, and syntax the parser could not read.

## Every language

Flow reads every text file in a repo, in any language, including languages it has never heard of. Files git ignores stay out, so two clones of a commit read the same files. A file gets the best understanding available:

The universal reader needs no grammar. It finds what nearly all languages share: blocks at the left margin, brackets and indentation that keep them open, comments and decorators above them, members of classes at any depth, names followed by `(`, and code inside strings like `${x()}`. Flat code with no layout splits at blank lines, labels and definition keywords. Import lines are read in any language, and a module matches a file by the end of its path, one rule for `./x`, `@/lib/x`, `crate::a::b`, `a.b.c` and `github.com/me/app/x`. A link made by matching a name alone is labelled "linked by name only" and counted in `_index.yaml`.

A grammar reader follows real types and exports. TypeScript and JavaScript have one today. `flow accuracy` scores the universal reader against it on any TypeScript repo. Functions inside functions stay with their parent, because they only run as part of it.

Generated and minified files are read as one unit. Lockfiles are read for what they mean: which package moved to which version, and which journeys use it. Binaries are tracked by content and taken whole by git. Text files over 30MB are not read.

## Scenarios

A scenario is any command, in any language, that runs a journey and prints what happened, as JSON or text:

```toml
[[scenario]]
name = "upgrade"
journey = "POST /api/upgrade"
run = "node scenarios/upgrade.mjs"
```

`flow run` runs them and compares with the recorded outcome; `flow run --record` records it. When a merge meets in a journey, flow checks out four real versions (where the branches split, each side, and the merged tree), runs the scenario on each, and compares every value. It reports values both sides decide, values that appear only when merged, and merges that fail when neither side did. Values that differ between two identical runs, like timestamps, are ignored. A disagreement blocks `flow save` until someone changes the code or runs `flow accept <scenario>`, which records the result so it is checked from then on. If the code changes during the merge, `flow save` and `flow accept` run the scenario again on the folder as it is, so a fix clears the block, and an acceptance only stands while the code gives that same result. When both sides decide a value, changing the code so it does what one side does settles it. A meet with no scenario stays a warning, and flow says so. Installed dependencies are shared into the checkouts; databases and outside services are whatever the scenario sets up.

## One comment per change

`flow comment` says in a few lines which journeys now behave differently, and flags the ones nobody asked for. Nobody writes or maintains anything:

```
Asked:     Mask the misspelling telegramm in guest messages
Not asked: Referrers now get 80% of their share, rounded down.
           Is that what you want?
             flow decide "GET /api/cron/booking-fees" yes   or   flow decide "GET /api/cron/booking-fees" no "give referrers their full share"
Changed:   Guests can no longer write telegramm in a message.
             reaches 2 journeys: action sendGuestMessage, action sendOwnerMessage
Kept:      no earlier decision was touched
```

What was asked comes from `--asked`, `--asked-file`, the pull request description in GitHub Actions, or the prompts your coding agent was given since the last save. With none of those there is no question: the comment lists what changed, most significant first. A change that reaches many journeys the same way is one line and one question, and one answer covers them all. Journeys wanted differently stay apart, even when the same value changed in both. `--full` unfolds everything.

A journey that reads a constant counts as reaching it: changing `MAX_NIGHTS` shows up in every journey whose code reads it, directly or through another constant. Comparing with no agent is done by words. A journey counts as asked if the request names it, words most journeys share do not count, and the rarest matching word decides.

`flow decide` saves the answer as one line in `decisions.md`, a plain file in the repo. The trailing HTML comment ties the line to the journey and to what it does:

```
- Annual customers who upgrade then cancel are refunded the full price. (2026-11-04) <!-- flow: POST /api/cancel | open -->
- Promo and loyalty credit do not stack. (2026-11-03)
```

Yes keeps what the journey does now, and the question is not asked again until it does something else. No says what it should do, and stays open on every change to that journey. If a later change alters a decided journey, the comment says so under Touched, and deleting a decision is reported under Removed. A line written by hand has no journey attached, so without an agent it shows under May touch when it shares two distinctive words with a change. Two branches adding decisions merge without a collision.

`flow diff` draws the same change for the browser: each touched journey as its steps in order, changed steps highlighted with before and after, untouched steps quiet, and the code one click behind each step.

## Your own agent

Git never involves AI, but flow can. It uses the `claude` or `codex` command already on your machine, on your own subscription, with no key of its own. On a machine with neither, such as a CI runner, flow reports plain facts and matches words. Choose with `ai = "claude"`, `"codex"` or `"off"` in `flow.toml`, or `FLOW_AI`.

`flow word` has the agent say a change in one plain sentence and judge it against the request. The results go in `flow.words.jsonl`, one per line, which is committed and reviewed like `flow.names.json`. The comment only reads that file and never calls anything, so the same saved change always gets the same comment. Three things keep the sentences honest:

- A line is keyed by the journey's behaviour before and after, so it can only describe the change it was written for.
- The agent describes the change without seeing the request, so it cannot repeat what was asked.
- Every number in a sentence must come from the code, or be the same rate as a percentage. A sentence that fails is refused and the plain facts stay.

A saved judgement outranks word matching, and it can flag part of a journey's change, so a request for longer stays that also doubled the booking window flags the booking window. Different agents can judge differently; the saved line is what counts, and a line with `"by": "person"` is never replaced. One sentence is written per distinct change, for at most 12 per run.

`flow hook install` tells flow when to fire. For Claude Code it adds hooks to `.claude/settings.local.json`, which stays on this machine. Each prompt becomes what was asked, and when a turn ends flow words the change in a process of its own that finishes even if the agent exits, and shows the comment. It waits at most 15 seconds, so a sentence that takes longer appears the next time the comment is shown. For Codex it prints the `notify` line to add yourself, since Codex allows one program across all repos; a turn's prompts and sentences are saved, and `flow comment` shows the result. A turn that changes no code, like a question, stays silent and adds nothing to what was asked. `flow save` also words the change first, so the sentences go in with the commit, unless you pass `--no-words`. Prompts stay in `.git` and never go into a commit.

Agents do not open `decisions.md` on their own, so the same hook hands Claude Code the decisions with each prompt, and it is told to raise a conflict before changing the code. Only the newest 4,000 characters go, and it is told how many older decisions the file holds. Codex has no hook for this, so `flow hook install` gives the line to add to `AGENTS.md`.

`flow why "reason"` is where the agent that did the work says why. Facts show what changed and the prompt shows what was wanted, but only that agent knows why it took this approach. The reason shows under the change, always labelled:

```
Not asked: POST /api/cancel
             Refund: value: 1 -> 0.9
             Agent's reason, not checked: refunds now match the upgrade discount
           Is that what you want?
```

It is the author's own account, so flow does not treat it as evidence. It is not held to the code, it never changes what counts as asked or decided, and it never replaces a fact. It is tied to the exact change, like a sentence, so if the code changes again it stops showing until a new reason is given. It goes to every journey the change touches unless `--journey` picks one. The Claude Code hook asks for it at the start of each turn; for Codex, `flow hook install` prints a line for `AGENTS.md`. Nothing in flow depends on it being there.

On a pull request, `--markdown` turns each question into checkboxes, and `flow answer <file>` records ticked boxes and replies like `/flow no <journey>: what it should do`. The same answer twice stays one line. These two workflows do both (a sketch: they have not been run on GitHub):

```yaml
# on: pull_request
permissions:
  contents: read
  pull-requests: write
steps:
  - uses: actions/checkout@v4
    with:
      fetch-depth: 0
  - run: cargo install --git https://github.com/andreachello/flow-control --locked
  - run: flow comment origin/${{ github.base_ref }} HEAD --markdown > comment.md
  - run: gh pr comment ${{ github.event.number }} --body-file comment.md
    env:
      GH_TOKEN: ${{ github.token }}
```

```yaml
# on: issue_comment (created, edited)
permissions:
  contents: write
  pull-requests: read
jobs:
  answer:
    if: github.event.issue.pull_request && (github.event.comment.user.login == 'github-actions[bot]' || contains(fromJSON('["OWNER","MEMBER","COLLABORATOR"]'), github.event.comment.author_association))
    env:
      GH_TOKEN: ${{ github.token }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - run: cargo install --git https://github.com/andreachello/flow-control --locked
      - run: |
          gh pr checkout ${{ github.event.issue.number }}
          jq -r .comment.body "$GITHUB_EVENT_PATH" > /tmp/answer.md
          flow answer /tmp/answer.md --base origin/main
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
          git add decisions.md 2>/dev/null || true
          git diff --cached --quiet || { git commit -m "Record decisions" && git push; }
```

The check on who commented matters on a public repo: without it, anyone could write your decisions.

## Commands

```
flow lift                  # write flows/
flow rename <old> <new>    # say one journey or step continues another
flow check                 # for CI: exit 1 if flows/ is stale or a line has no owner
flow show "POST /api/upgrade"
flow ledger                # files with unplaced lines
flow ledger lib/mailer.ts  # owner of every line
flow journeys              # Claude groups requests into what people come to do, in flow.journeys.json
flow titles                # Claude writes one short sentence per change, in flow.titles.json
flow name                  # Claude proposes plain names into flow.names.json
flow view                  # open the journeys in the browser
flow serve [repos...]      # your projects on localhost: changes, files and journeys
flow run [name] [--record] # run scenarios, compare with recorded outcomes
flow accept <scenario>     # accept what a merge does
flow comment [base] [head] --asked "..."     # one short comment on a change
flow decide <journey> yes|no ["sentence"]    # save the answer to decisions.md
flow word [base] [head]    # have your agent put a change into plain sentences
flow why "reason"          # say why a change was made, shown as unchecked
flow answer <file>         # record the answers in a pull request comment
flow hook install          # connect this repo to the agents on this machine
flow diff [base] [head]    # the change as journeys, in the browser
flow accuracy              # score the universal reader against the grammar reader
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

Git writes the result, so binary files, file modes and links come through exactly, and a merge refuses to overwrite a file git does not track. Files in `flows/` are never merged: they are rebuilt from the merged code. In a git repository, `flow lift` reads exactly what a save would record: tracked files plus new files that are not ignored. Line endings count the way git counts them, so a Windows checkout lifts to the same bytes.

A file renamed on one side and edited on the other keeps the edit at its new path. When both sides rename a file, their edits still merge. If they chose different names, that is a collision: `flow resolve <file> ours|theirs` picks the name, so the file never ends up in both places. `.gitattributes` rules `merge=binary`, `-merge` and `merge=union` are read from the commits being merged. A named merge driver is ignored, because it lives in someone's personal git config and a merge must not depend on that.

`flow status` and `flow explain` say what each change is, read from the two versions with no AI: values that changed, conditions, calls added or removed, data it now writes, and what a new function's comment says. Keys that changed in JSON, YAML, TOML and `.env` files are listed too. The same change always gives the same facts. `--full` lists every function.

## Plain names

`flow name` asks Claude (through the `claude` command, so no API key is needed) to name every journey and step in plain words and to sort journeys into areas such as "Owner dashboard". The names are saved to `flow.names.json`, which is meant to be committed and reviewed. Each name stores the hash of the code it describes: when that code changes, the viewer marks the name as stale and `flow name` proposes a new one. To correct a name, edit the file and set `"by": "person"`; `flow name` never overwrites those. Names never change what a flow contains.

`flow view` opens every journey in the browser with those names, and falls back to names guessed from the code.

## Journeys and requests

A page, an action and a route are each one request. A journey is what those requests add up to: the thing a person came to do, like "Add a house" or "Upgrade to Pro". The code shows which requests lead to which, but not what anyone is trying to do, so `flow journeys` asks Claude to group them into at most 6 journeys, and anything no person starts goes in "Behind the scenes". The result is `flow.journeys.json`, meant to be committed and read: rename a journey or move a request by editing it.

It also writes each journey out as its stops, what a person does from start to finish, so a change can be shown at the place in the journey it reaches. Running it again only fills in what is missing, placing new requests in the journeys you already have, so titles and colours stay put. A request stays listed after its code is gone, so old changes still read as the journey they belonged to. Nothing asks Claude when a page loads, and the lifter never reads the file: a journey only groups what the lift found. Until the file exists, `flow serve` shows single requests and says so.

`flow titles` does the same for the list of changes: one short sentence per commit in `flow.titles.json`, or "Nothing changes for people." It reads each diff, so every point can say how things were and how they are now, at the step of the journey where a person meets it. Edit any of them by hand. A commit without a title shows its message.

`flow journeys` also draws how the journeys fit together: who uses the product and which journeys each kind of person goes through, in order. It is saved as `map` in the same file, shown at the top of the Journeys page, and yours to edit. Delete `map` and run it again to redraw it.

## In the browser

The hosted product is the Next.js app in `web/`, in front of `flow serve` as its engine. Next answers on port 4000, with Soba's own components, and `flow serve` answers only on this machine:

```bash
cd web && npm install && npm run build
flow serve --port 4001 --public-port 4000 &
npm start
```

The screens are moving into React one at a time. Until they have, `web/scripts/split-page.mjs` reads them from `src/serve.html`, so plain `flow serve` keeps working on its own.

`flow serve` shows your projects on `http://localhost:4000`: every project with recent activity across them, then for each one an overview, its pull requests, its journeys, its changes and the people behind them. A journey opens to its history and the requests it runs, and a request to the steps it runs and the data it touches. A change opens in plain words: what is different for people and which journeys it reaches. Its advanced view has the files and diff, step by step. `⌘K` searches everything, and `?` lists the keyboard shortcuts.

```
flow serve                              # the repository you are in
flow serve ~/Coding/shop ~/Coding/blog  # and these, as a list of projects
flow serve --port 4100
```

It reads: everything comes from git and from lifts in memory, and it answers only on this machine. A change is shown as `git diff` shows it, so renames are detected the way git detects them, while journeys come from the lifts. The list comes from git at once, and what each change reached follows a moment later, because working that out takes two lifts. Next.js routes are only found at the top of a repo, so a monorepo's `apps/web/app/` shows server actions but not its pages and routes.

Pull requests come from the `gh` command, so your own GitHub sign-in decides what you can see, and a project off GitHub just says so. One opens to the journeys it changes, step by step, from the changes it holds. `flow serve` never fetches: a change that is not on this machine is counted and left out until you run `git fetch`.

Where the page would otherwise send you to a terminal, it has a button instead. **Find journeys** runs `flow journeys` for that project, and **Write the changes** runs `flow titles`. They use the agent signed in on the machine running `flow serve`, one run per project at a time, and write the same files the commands do, for you to review and commit. The rest of `flow serve` only reads. A run keeps going if you leave the page, and with sign-in on, anyone allowed in can start one.

### Runs through Soba

With sign-in on, a run from the page can use the computer or plan the signed-in person connected through [Soba](https://soba.so), instead of the agent on the machine running `flow serve`. Link an app with Soba's CLI, in the folder you start the server from:

```bash
npx @soba-so/cli login --no-link
npx @soba-so/cli link --workspace "<workspace>" --app "<app>"
flow serve
```

`flow serve` reads `SOBA_KEY` from that folder's `.env.local`, or from the file you name with `--env-file`. It never reads a project's `.env.local`, because that key belongs to that project's own app. On a host that keeps its own secrets, `SOBA_KEY` in the environment wins.

Each person connects their own computer with **Connect compute** at the top of the page, and runs use what they connected. Runs go through Soba's SDK, so they are encrypted on this server for that person's own machines: Soba routes them and cannot read the code in them. That also means a connected plan on its own is not enough, because a run needs a machine holding a key. Commands typed in a terminal keep using the local agent, and so does the page with sign-in off, because nobody is named.

The SDK needs Node 22 or newer. With a key set, `flow serve` installs a pinned copy into `~/.flow` the first time it starts, and refuses to start without Node.

### Sign in with GitHub

`flow serve` is open to this machine unless you turn sign-in on. To turn it on, create an OAuth app at [github.com/settings/developers](https://github.com/settings/developers) with the callback `http://localhost:4000/auth/github/callback`, then start the server with its keys and the usernames who may sign in:

```bash
export FLOW_GITHUB_CLIENT_ID=...
export FLOW_GITHUB_CLIENT_SECRET=...
export FLOW_ALLOWED_USERS=your-username,a-teammate
flow serve
```

Flow asks GitHub for no permissions, reads the public profile once and drops GitHub's token. Sessions last a week and end when the server restarts. The server still only listens on this machine: signing in does not make it safe to expose on its own.

## Permanent ids

A journey's name comes from the code, so moving a route changes it. Its `uid` does not. `flow history`, `flow status`, `flow comment` and `flow affected` pair journeys by `uid`, so a route moved from `/api/charge` to `/api/pay` reads as one journey renamed, with its history, decisions and plain names intact. `flow history` finds it by either name.

An id is the hash of the name the journey or step was born with, so every clone and branch gets the same id with nothing stored, and a repo that never renames anything has no extra file. Renames go in `flow.ids.jsonl`, which is committed and reviewed. It is written by `flow lift` and `flow save`, never by `flow status` or `flow comment`, which only show what they would record. Merges combine it as a set of facts, so a merge gives the same result in either direction.

A rename is recorded only when the code proves it, by comparing with the last saved snapshot:

- The same code under another name, when that is the only possible match.
- The same name in another file, when that is the only possible match and at least half the lines are shared. A shared name alone proves nothing: every route handler is called POST.
- A journey whose entry point was linked this way, or stayed put under a new `id` in `flow.toml`.

Anything else stays one journey gone and one new, and `flow lift` says so. `flow rename <old> <new>` settles it by hand, for a journey or a step.

A name given up can be reused: a new route at the old path is a new journey with its own id. A decision written against that name then applies to the new route, not the renamed one, because a name in use comes first. A scenario still naming an old journey is reported in `_index.yaml`, since `flow.toml` is yours to edit.

A rename committed with plain `git commit` skips the comparison, so it shows as gone and new. `flow check` fails on that commit because `flows/` is stale. Run `flow rename <old> <new>` to rejoin the history from then on; the commits in between keep the id they were lifted with. Renames from before this feature are not rejoined automatically either.

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
2. Run every journey on two commits and report what each one now does differently.
3. Collisions: run each change alone and merged to find where changes decide the same thing.
4. A local web UI built from the prototype screens: the journey player, before and after, and collisions.

`cargo test` runs the parser tests and the demo fixture in `fixtures/demo`, a TypeScript port of the prototype app.
