# Import from T3 Code

`tcode-headless import-t3` imports T3's local v2 projection database into Tcode's
JSON/JSONL store. Close the destination Tcode desktop app and headless host first.
T3 can remain open: discovery reads one consistent, read-only SQLite transaction.
The importer does not start providers, send prompts, or modify native histories.

```sh
cargo run -p tcode-headless -- import-t3 \
  --source ~/.t3/userdata \
  --data-dir ~/.local/share/tcode \
  --profile Openrouter=openrouter \
  --dry-run
```

Remove `--dry-run` to apply. The source defaults to `~/.t3/userdata`; the destination
uses `TCODE_DATA_DIR`, then Tcode's platform data directory. Dry-run validates the
schema, profile mappings, matching and history conversion without creating the
destination or repairing its index. Counts describe planned changes in dry-run.
Conversion failures abort before writes. Write failures are reported per thread;
completed imports can be refreshed by rerunning the command. Configuration,
conversion and write failures return a nonzero exit status.

`--profile SOURCE=DESTINATION` is repeatable. SOURCE is the provider instance ID
from T3's `settings.json`, such as `Openrouter`. DESTINATION is an existing Tcode
profile ID, such as `openrouter`, with the same provider protocol. Configure custom
profiles in Tcode before closing it. T3's built-in `codex` and `claudeAgent`
instances use Tcode's built-in Codex and Claude Code profiles unless mapped
explicitly. Credentials and provider configuration are never copied.

All eligible projects are imported, including empty ones. Projects are matched
by canonical directory path. New projects keep T3's names and creation times;
existing Tcode project names remain unchanged. Thread titles, timestamps, models,
supported model options, archive timestamps and explicit manual settled state
are retained. Both archived and settled threads are included by default;
`--skip-archived` and `--skip-settled` independently exclude them. Intentional
exclusions are successful outcomes and are counted by reason.

Repeated imports match `t3code:<thread-id>` and keep the destination thread ID.
They replace the imported history, title, model, resume cursor and status with
T3's current version, including any edits made to that thread in Tcode. Earlier
Codex/Claude imports can be adopted by native session identity. Native
Tcode-created conversations and ambiguous matches are reported as conflicts.
Filtered, deleted or missing source records never delete earlier imports.

History comes from canonical T3 messages and activity, ordered by run and item
position with referenced messages deduplicated. Commands, tools, file diffs,
searches, subagent summaries, tasks, plans and compaction use Tcode's timeline
format. Historical approvals, input requests and checkpoint operations are
readable summaries; they cannot execute. Imported proposed plans have no pending
accept/refine action. Unfinished runs appear as interrupted.

Deleted projects and threads, missing project roots or thread directories, and
threads without a valid active native session ID are excluded. Existing worktree
paths remain the working directory without becoming Tcode-owned worktrees.
Forked conversations are included; subagents stay in parent activity. Attachments
are counted and omitted, without fetching or copying files. Message text remains
as stored. Running jobs, queued actions, terminal sessions, checkpoints, snoozes,
and inferred settled state are not restored. Only the supported v2 projection
schema is accepted, including history T3 has already migrated from v1.

For inspection without changing your normal profile, use a separate `--data-dir`
and open Tcode with `TCODE_DATA_DIR` pointing to that directory. Continuation
requires the configured provider's native session history to remain available.
Opening imported conversations does not start a provider; sending a message does.
