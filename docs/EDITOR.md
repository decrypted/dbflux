# Running Queries

Open a new query tab with `Ctrl+n` (`Cmd+n` on macOS), or open a script file with
`Ctrl+o`. The editor's query language (SQL, MongoDB query syntax, Redis commands,
etc.) is determined by the active connection's driver, which also drives syntax
highlighting and the placeholder text. A `.csv` or `.tsv` file opens as a table
instead; see [CSV and TSV Files](CSV_FILES.md).

For SQL connections, the [Visual Query Builder](QUERY_BUILDER.md) composes
queries without writing SQL.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/editor/query-result-dark.webp">
  <img src="images/editor/query-result-light.webp" alt="A query tab with a SQL query that groups orders by country, and its result grid">
</picture>

## Saving and closing tabs

A new query tab (`Ctrl+n`) is backed by a real file in your scripts folder, the
same way a script opened with `Ctrl+o` is. Open editors auto-save to that file
on the configured interval, and `Ctrl+s` / **Save file as…** go through the same
queue. Autosave and closing never overwrite a file that changed outside DBFlux:
your version stays in the editor and DBFlux reports the refused write. `Ctrl+s`
and **Save file as…** are deliberate and write the file even then.

Closing a tab with pending edits saves them first, then closes; if the write
cannot land (for example, the file changed outside DBFlux or is read-only), the
tab stays open with your changes and DBFlux points at `Ctrl+s` / **Save file as…**
as the deliberate overwrite. A buffer with no file yet is the exception: closing
it asks first, so you can save it, close it without saving, or cancel. Quitting
DBFlux saves pending edits the same way before it shuts down. If the scripts
folder could not be created at startup, new queries are kept in the session store
instead, and **Save file as…** is offered when you close them.

## Executing

- `Ctrl+Enter` (`Cmd+Enter`) — **Run query**.
- `Ctrl+Shift+Enter` (`Cmd+Shift+Enter`) — **Run query in new tab**.

If a non-empty text selection exists, only the selected text runs. With no
selection and several `;`-separated statements in the buffer, only the
statement under the cursor runs: a cursor after a statement's `;` or in the
blank lines below it runs that statement, and a cursor above every statement
runs the first. Comment lines directly above a statement belong to it, so a
cursor on them runs that statement, while a comment after a `;` on the same line
belongs to the statement it follows. Each statement's run marker sits on its
first line of code. To run the whole buffer, select all (`Ctrl+A`) first. A buffer
with a single statement, or in a language without statement splitting, runs
whole. So does a buffer with a compound block, such as the `BEGIN ... END` body
of a procedure or trigger, since the `;` inside it does not end a statement;
`BEGIN` that opens a transaction is not a block.

Without a selection, when the statement under the cursor only opens or ends a
transaction (`BEGIN`,
`START TRANSACTION`, `COMMIT`, `END`, `ROLLBACK` or `ABORT`) and the buffer
holds other statements, the editor asks first: run the entire script, which is
the default, or run only that statement. A connection whose driver cannot run
a whole script at once offers only that statement. Run alone, a `BEGIN` leaves a
transaction open that later runs in the same tab silently join, and a `COMMIT`
or `ROLLBACK` skips the rest of the script, ending at most a transaction an
earlier run left open.

Each result is captioned with the lines of the statement that produced it. A
statement that returns no result set, such as `UPDATE` or `COMMIT`, shows
**Statement executed** and, when the driver reports it, how many rows it
affected, which the footer repeats in place of the row count. Notices and
warnings the server sends while the statement runs, such as PostgreSQL's
`WARNING: there is no transaction in progress` after a `COMMIT` outside a
transaction, are listed under that line, or in the footer of a result with rows.
Only drivers that report them show them; PostgreSQL does.

When execution actually omits rows, the editor reports one warning for the query and the grid marks the affected result set, even if no rows were retained. A result that exactly fills a limit without omitting rows does not trigger the warning. A retained-row cap limits stored rows only; byte and time limits are separate execution controls. This does not imply a default editor row cap.

### Explaining a statement

**Explain** shows how the database would run a statement, without running it.
It is the info button on the editor's action strip, the Explain entry of the
pane actions menu and of the editor's right-click menu, and the **Explain query**
command in the palette; `Leader e` in Vim's Normal mode.

It plans what a run would execute: the selection, or without one the statement
under the cursor, by the rules above. A selection of several statements is
refused with a message and nothing is sent, since no single plan answers it, and
so is a buffer with a compound block. A statement that only opens or ends a
transaction has no plan to show.

The plan comes from the driver and the statement never runs, so a write is
planned without a confirmation, on a read-only connection too:

| Database | Asks for the plan with |
|---|---|
| PostgreSQL, MySQL, MariaDB, DuckDB, Redshift, ClickHouse | `EXPLAIN` |
| SQLite, Turso | `EXPLAIN QUERY PLAN` |
| SQL Server | `SET SHOWPLAN_TEXT ON` |

There is no `ANALYZE`, which would run the statement. A statement you already
wrote as an `EXPLAIN`, such as `EXPLAIN (COSTS OFF) SELECT ...`, runs as written.
One that would run what it explains, such as `EXPLAIN ANALYZE ...`, is refused:
use Run for it. SQL Server has no `EXPLAIN` statement: it would read the keyword as
a procedure call and then run the rest, so it refuses any request that starts
with `EXPLAIN`. ClickHouse evaluates scalar subqueries while it plans, so a plan
there can read data; writes are refused.

Explain is offered for SQL only. On a connection whose driver has no explain it
says so and sends nothing. The result is captioned **Explain** followed by the
statement's caption. A plan made of text lines, or with a cell of many lines,
opens in the Text view with its indentation intact; a plan that is a real table,
as on MySQL and SQLite, opens in the grid. The query history records it as
`EXPLAIN` and the statement, so picking it there plans again.

## Multi-statement scripts

To run several statements as a script, select them (`Ctrl+A` selects the
whole buffer) and press `Ctrl+Enter`. When the active driver advertises batch
support, each statement's result set is rendered in its own result tab.

Statement splitting is language-aware for SQL-family languages: separators inside
strings, identifiers, line/block comments, and PostgreSQL dollar-quoted bodies are
not treated as statement boundaries. A part that holds only comments, such as a
closing note after the last `;`, is not a statement and is not sent to the
database. Non-SQL languages remain single-statement.
Batch support is per-driver — among the built-in SQL drivers, PostgreSQL,
MySQL/MariaDB, SQLite, and Microsoft SQL Server support it. A selection always
runs as-is.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/editor/multi-statement-dark.webp">
  <img src="images/editor/multi-statement-light.webp" alt="A query tab that ran three statements as a script, with one result tab per statement">
</picture>

## Dangerous-query confirmation

DBFlux detects dangerous operations across languages — SQL `DELETE`/`DROP`/
`TRUNCATE` and `DELETE`/`UPDATE` without a `WHERE`, MongoDB `deleteMany`/`drop`,
Redis `FLUSHALL`/`FLUSHDB`/`KEYS` — and prompts for confirmation before running.
This behavior is governed by settings: dangerous-query confirmation can be turned
off, a `WHERE` clause can be required for `DELETE`/`UPDATE`, and Redis
`FLUSHALL`/`FLUSHDB` can be disabled entirely (in which case those commands are
blocked rather than confirmed).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/editor/dangerous-query-dark.webp">
  <img src="images/editor/dangerous-query-light.webp" alt="The dangerous-query confirmation for a DELETE without a WHERE clause">
</picture>

## Scripts (Lua / Python / Bash)

Lua, Python, and Bash documents execute as scripts rather than database queries.
Their output streams live into the document's output area while running, and the
final output is kept as a text result. See `docs/LUA.md` for the embedded Lua
runtime.

## Saved Queries and History

DBFlux keeps a history of completed queries and lets you save named queries.

- `Alt+h` (in the editor), or the History button in the action strip on the
  editor's left edge, opens and closes the query history panel beside the
  editor.
- `Ctrl+s` (`Cmd+s`) — **Save** the current query.
- `Ctrl+Shift+s` (`Cmd+Shift+s`) — **Save file as…**.
- `Ctrl+p` (`Cmd+p`, in the editor) — open the saved-queries browser.

The history panel lists recent and saved queries and stays open while you edit.
Clicking an entry or pressing `Enter` loads it into the editor. While the panel
has focus you can navigate with `Ctrl+j`/`Ctrl+k` (or arrow keys), save a recent
query with `Ctrl+s`, and use the local mnemonics `Ctrl+f` (toggle favorite),
`Ctrl+r` (rename), and `Ctrl+d` (delete). `/` or the search button in the panel
header opens the search field, and `Esc` closes the panel.
