# Working with Results

Results render in result tabs inside the document. Each table or collection
opens in the view its driver declares for that object, and offers the other
views it declares:

- **Table view** for relational tables and query results.
- **Tree view** for document collections (for example MongoDB, DynamoDB), with
  **Table** and **JSON** views of the same page (see
  [Document collections](DOCUMENTS.md)).
- **Table view** with a **Tree** view of the same rows for a table with JSON
  columns (see [JSON columns in a table](DOCUMENTS.md#json-columns-in-a-table)).
- **Chart above the grid** for a time series, such as an InfluxDB measurement or
  a TimescaleDB hypertable, when its rows can be charted (see
  [Time-series tables and collections](CHARTS.md#time-series-tables-and-collections)).
- **Key-value view** for Redis (see [Key-Value Browser](KEY_VALUE.md)).

A table with a vector column also lists the rows most
[similar](#similar-rows) to the one selected.

Event-stream-style containers open as event streams when the driver declares that
presentation.

## Results beside the editor

In a query tab the results sit below the editor. On a wide screen, move them to
the right of the editor with the panel button in the results header, the
**Show results on the right** entry of the pane actions, or **Move results
beside or below the editor** in the command palette. The same control moves
them back below. Drag the divider to resize the two sides. New query tabs open
with the position you chose last.

In either position, `Ctrl+j` moves the keyboard from the editor to the results
and `Ctrl+k` moves it back.

## Navigating the data grid

When the results panel has focus:

- `j`/`k` (or `Down`/`Up`) — move between rows.
- `h`/`l` (or `Left`/`Right`) — move between columns.
- `g`/`Shift+g` (or `Home`/`End`) — first / last row.
- `Ctrl+d`/`Ctrl+u` (or `PageDown`/`PageUp`) — page through rows.
- `[` / `]` — previous / next page of results (pagination).
- `f` focuses the toolbar; `/` focuses the search/filter.
- `z` toggles collapsing the panel.
- `m` (or `Shift+F10`) opens the row/cell context menu.

With the mouse, a click selects a cell and `Shift`+click, or pressing and dragging, selects the range from it to another cell. A click on a row number selects the whole row, and `Shift`+click on another row number selects every row in between. `Ctrl`+click (`Cmd`+click on macOS) on a cell or a row number adds it to the selection, or takes it out when it is selected on its own; copying several ranges copies the rows and columns they cover, with cells outside the selection left empty. The extra ranges are for copying: delete, duplicate, save and the other row actions act on the row of the cursor, a right-click selects the cell under the pointer, and the record view treats `Ctrl`+click as a plain click. Dragging past an edge of the grid scrolls it that way, faster the further out the pointer is, and keeps extending the range until the button is released or the pointer comes back.

When the selection covers more than one cell, the status bar shows how many cells it holds and, for the numbers among them, their sum, average, minimum and maximum. NULLs and text are counted but left out of the figures, and the status bar says how many it skipped. The totals are worked out from the rows already loaded, unsaved edits included, without querying the database again.

## Record view

Press `i`, or use the Record toggle in the result status bar, to show
the active row as a Name / Value list that fills the result area. The header
names the row's position in the result. Fields are edited exactly like grid
cells, so unsaved changes, Save Row and revert work the same in both layouts;
`Up`/`Down` move between fields and `Left`/`Right` move between rows. Press
`i` again to return to the grid.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/results/record-view-dark.webp">
  <img src="images/results/record-view-light.webp" alt="The record view showing one customer row as a list of fields and values">
</picture>

## Column header menu

Right-click a column header for a menu scoped to that column: order ascending
or descending, clear the ordering, and every filter operator, in one flat
list. A left click on the header selects the whole column (`Shift`+click selects every column up to it, `Ctrl`+click, `Cmd`+click on macOS, adds or removes it); the arrow at the header's right edge cycles the sort, and shows a faint up/down arrow while the column is not sorted. With **Clicking a column header sorts the column** on (Settings > General > Data grid), a plain click on the header sorts it instead, as the arrow does, and `Alt`+click on the header adds a sort key as on the arrow.

To sort by several columns, `Alt`+click the arrow of another column: it becomes
the next sort key, ascending, and a further `Alt`+click turns it descending and
then takes it out of the sort. With more than one key, each sorted header shows
its place (1, 2, 3) next to the arrow, and rows that tie on the first key are
ordered by the second, and so on. A plain click on any arrow sorts by that
column alone again, in the direction it already had when it was a key. A table
whose primary key has several columns opens sorted by all of them, so its key
columns show 1 and 2 from the start. Some Linux desktops (KDE Plasma, for one)
take `Alt`+click for moving windows; there the column menu's **Add to sort**
does the same, or the window manager's modifier can be changed. The column menu offers the same as **Add to sort: column
ASC** and **Add to sort: column DESC**, which also change the direction of a
column that is already a key, and **Remove ordering** drops every key. A table
sends all keys to the database as one `ORDER BY`; a query result is sorted in
memory. On a table still in its default order (its primary key), the first
added key replaces that order, since rows sorted by the primary key first have
no ties left for another column to order; removing the ordering returns to the
default order.

`Shift+m` opens the same menu from the keyboard, for the column of the selected
cell. The menu ends with a **Column** section of view settings, which change
how the grid shows the column and never the query. **Fit width** sizes the
column to its header and its loaded values, up to 640 pixels so one long JSON
value does not push the rest of the grid off screen; a double-click on the
resize edge at the header's right border does the same.

**Hide column** takes the column off the screen, and **Columns…** lists every
column with a check mark on the shown ones: `Enter`, `Space` or a click shows
or hides the highlighted column while the list stays open, **Show all
columns** brings every hidden one back, and `Escape` closes it. **Move left**
and **Move right** move the column one place; dragging a header onto another
header moves its column into that place.

**Pin column** keeps the column at the left edge, right after the row numbers,
while the grid scrolls sideways, like a frozen pane: pin `id` and `created_at`
to keep them in view on a wide log table. Pinned columns lead the grid in the
order they were pinned and show a pin in their header; **Unpin column** lets
the column scroll again, as the first of the others. Selecting, editing,
sorting, resizing and the row numbers work the same on pinned columns. Move
left and Move right stay on their side of the pinned edge; dragging a header
across it pins or unpins its column. Pinned columns take at most two thirds of
the grid's width, so the other columns always keep room: in a narrow grid the
pinned columns past that scroll with the others until it is wide enough again.

The query keeps reading every column (a table still runs `SELECT *`); to leave
a column out of the query itself, use the query builder's projection. Copy,
Copy as CSV and Copy as JSON follow the screen: the shown columns, in the order
the grid shows them. Copy row, Copy as Query and Export use the whole row in
the result's own order, hidden columns included. A table remembers its layout
for its connection, database, schema and table, across closing it and
restarting DBFlux; moving every column back to the table's own order with all
of them shown forgets it. A query result keeps its layout while its tab is
open. A document grid keeps the order of its fields and offers only Fit width.

## Value panel

Right-click a cell and choose **View Value**, or press `v`, to open the cell
in the inspector rail on the right. The panel shows the value as JSON, XML or
plain text — detected from the content, and only when it really parses — with
pretty-print, compact and word wrap. You can edit there: **Save** commits the
row directly, **Revert** discards the edit. The panel follows the selected cell
as you move through the grid, except while it holds an unsaved change.

A binary cell (`bytea`, `BLOB`, `varbinary` and the like) opens as a hex dump,
read-only. When its bytes are a PNG, JPEG, GIF, WebP or BMP image, recognized
from the bytes themselves and not from the column name, the panel shows the
picture instead, scaled down to fit, with its pixel size, format and byte size
under it. **Hex** switches to the hex dump and **Image** back. **Save image
as…** writes the stored bytes to a file and **Copy image** puts the image on the
clipboard; both are also Toolbar entries of the grid's context menu (`m`). A text
cell holding an image as base64, bare or as a `data:image/...;base64,` URI,
opens on the picture the same way, with **Text** to switch back to the text.

Images above 32 MB or 32 megapixels are not decoded, and neither is a value that
ends before its image does, as one cut short by a size limit would: the panel
says why above the hex dump. Animated images show their first frame, and SVG is
not drawn. The hex dump of a large value stops after its first 64 KB.

## Row inspector

Press `Ctrl+Space`, or right-click a row and choose **Inspect Row**, to open
the selected row in the inspector rail on the right. It lists every column of
the row with its value and marks primary and foreign key columns. The inspector
follows the selected row as you move through the grid; the pin button in its
header keeps it on the current row instead. **Edit**, **Duplicate** and
**Delete** at its foot act on the inspected row when the result is editable.
Press `Ctrl+Space` again, or the close button, to dismiss it.

Under **References** the inspector lists the row's foreign keys in both
directions:

- **Outgoing** — a single-column foreign key of this row points at another
  table. The entry names that table and the row it points at, by its readable
  label once resolved ("María López · #8"), or by its key until then. Click it
  to open the referenced table on that row: DBFlux reuses the table's existing
  tab — opening one if none is open — and replaces its filter with a
  primary-key equality, so the referenced row is the one in view.
- **Incoming** — other tables hold foreign keys that point at this row. Each
  entry names the table and counts how many of its rows reference this one;
  counts load in the background. These entries are informational only.

Foreign-key cells in the grid can show the same referenced-row label next to
the id: turn on **Show referenced-row labels in foreign-key cells** in
[Settings](SETTINGS.md). The row inspector resolves references regardless of
that setting.

## Filtering results

The data grid toolbar has a `WHERE` filter input that re-runs the query with the
condition you type. For SQL connections it supports two styles:

- **Raw `WHERE`** — type a plain condition (for example `status = 'active'`). This
  is the default behavior.
- **Relational (ORM-style) paths** — type a dotted path that walks foreign keys,
  for example `created_by.email LIKE '%@acme.com'` or
  `created_by.organization.name = 'Acme'`. DBFlux resolves the path against the
  table's foreign-key metadata and joins through to the referenced table for you;
  there is no need to write the JOINs by hand.

When a relational filter resolves, a chip shows how many joins it added. If a
segment is ambiguous or cannot be resolved, an inline error appears with an
**Open in builder** link that opens the visual query builder seeded with the
joins resolved so far. Non-dotted input always keeps the raw-`WHERE` behavior.

The filter input also offers schema-aware autocomplete (same navigation as the
builder — see [Schema-aware autocomplete](QUERY_BUILDER.md#schema-aware-autocomplete)).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/results/filtering-dark.webp">
  <img src="images/results/filtering-light.webp" alt="The customers table filtered by a WHERE condition typed in the filter bar">
</picture>

## Similar rows

On a table with a vector column (today a PostgreSQL table with a pgvector
`vector` column), press `Shift+s` or choose **Find similar rows** in the row
menu to list the rows nearest to the selected row. The menu entry names the
column it reads and how many rows it lists, or why it cannot run: the row has no
value in that column, or the grid holds unsaved changes, which must be saved or
reverted first.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/results/similar-rows-dark.webp">
  <img src="images/results/similar-rows-light.webp" alt="The products table after Find similar rows: the selected row pinned on top as the anchor, its ten nearest rows ordered by L2 distance, and the bar with the vector column, the distance and K">
</picture>

The grid then shows the selected row pinned on top, marked `anchor`, and its 10
nearest rows below it, closest first. The first column holds the distance, under
a header that names it. A bar replaces the filter row:

- The column picker chooses the vector column. With the cursor on a vector
  column, the search starts on that column; otherwise on the table's first one.
- **L2**, **Cosine** and **Inner product** switch the distance. Inner product
  shows the product itself, highest first.
- **K** sets how many rows to list, from 1 to 1000.
- **Back to table**, or `Esc`, returns to the table page with its filter.

The selected row is left out of its neighbors by its primary key, rows without a
vector are not listed, and similar rows are read-only. A distance that is not a
number shows a dash. `halfvec` and `sparsevec` columns do not offer the search.

## Editing and CRUD

In the data grid:

- `o` — add a row.
- `x` — delete the selected row.
- `r` — rename / edit (context-dependent).
- `y` — copy the selected row.
- `Ctrl+c` (`Cmd+c`) — copy the selected cell(s) to the clipboard.

With several cells selected, `Enter`, `F2` or a double-click on one of them opens the editor for all of them, and the status bar says how many cells the edit will change. The value you commit is written to every selected cell of the same column type that can take it, as unsaved changes that Save and Revert handle like any other edit, and one undo (`u` or `Ctrl+z`) takes the whole edit back. Cells in read-only columns, rows marked for deletion and values the inline editor does not edit (JSON, long or multi-line text) are skipped, and so are key columns when the selection spans several rows, since one value in every row's key could never be saved; a value picked from a column's list of allowed values goes only to columns with the same list.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="images/results/editing-dark.webp">
  <img src="images/results/editing-light.webp" alt="The customers table with one edited cell not yet saved, and the Save and Revert buttons enabled">
</picture>

### Editing long values

JSON columns and long values open in the modal cell editor. A text cell whose
content parses as a JSON object or array, or as XML, opens there in the same
editor a JSON column gets, detected the same way the value panel detects it:
pretty-printed, with syntax highlighting, line numbers and folding, a status
line that says whether the text still parses, and **Format** and **Compact**.
The title names both the column type and the format, for example
`Edit raw_result (TEXT · JSON)`. Bare numbers and strings stay plain text.

Showing a value pretty does not change how it is stored. A value stored on one
line is saved compact again, with only the whitespace between tokens removed,
so key order, numbers such as `2312.64496440` and strings such as
`"2725.8328350479"` stay exactly as written. A value stored over several lines
is saved as edited, and **Save** without a change creates no pending edit. If
the text of a text column no longer parses, the first **Save** shows a warning
and the second saves it as typed; a JSON column refuses it.

**Copy** puts the editor's text on the clipboard as shown, unsaved edits
included, and **Copy compact** copies it compacted. Drag the editor's right or
bottom edge or its bottom-right corner to resize it, or maximize it with the
button next to the close button. The size is remembered for the next value.

| Keys | Action |
|------|--------|
| `Ctrl+s` (`Cmd+s`) | Save |
| `Ctrl+Shift+k` / `Ctrl+Shift+h` | Copy / Copy compact (`Cmd` on macOS) |
| `Ctrl+Shift+u` / `Ctrl+Shift+j` | Format / Compact |
| `Ctrl+Shift+Enter` | Maximize or restore the editor |

`Ctrl+c` (`Cmd+c`) keeps copying the selected text.
### SQL expressions in cells

In a table of MySQL, MariaDB, PostgreSQL, SQLite or SQL Server, a cell can be set to a SQL expression instead of a value, such as `NOW()`, `"qty" + 1` or `DEFAULT`. The expression is saved unquoted in the row's `UPDATE` or `INSERT`, and the database computes the value.

- **Edit as SQL expression…** in the cell's context menu, or `Ctrl+Alt+E` (`Cmd+Alt+E` on macOS), opens the editor with its `fx` switch on. On a cell of a larger selection the expression goes to every selected cell, as a multi-cell edit does.
- In an open editor, a click on `fx` or the same shortcut switches between a value and an expression, keeping what was typed.
- While `fx` is on, a list under the editor suggests expressions for the column's type and the database, and `Down` / `Up` write them into the editor one by one. Any other expression can be typed.

| Column type | MySQL / MariaDB | PostgreSQL | SQLite | SQL Server |
|---|---|---|---|---|
| Date and time | `NOW()`, `CURRENT_TIMESTAMP`, `UTC_TIMESTAMP()`, `DATE_ADD(NOW(), INTERVAL 1 DAY)` | `now()`, `current_timestamp`, `now() + interval '1 day'` | `CURRENT_TIMESTAMP`, `datetime('now')`, `datetime('now', 'localtime')`, `datetime('now', '+1 day')` | `GETDATE()`, `CURRENT_TIMESTAMP`, `SYSDATETIME()`, `GETUTCDATE()`, `DATEADD(day, 1, GETDATE())` |
| Date | `CURDATE()`, `CURRENT_DATE`, `UTC_DATE()` | `current_date`, `current_date + 1` | `CURRENT_DATE`, `date('now')` | `CAST(GETDATE() AS date)` |
| Time | `CURTIME()`, `CURRENT_TIME`, `UTC_TIME()` | `current_time`, `localtime` | `CURRENT_TIME`, `time('now')` | `CAST(GETDATE() AS time)` |
| uuid | `UUID()` (`char(36)`), `UNHEX(REPLACE(UUID(), '-', ''))` (`binary(16)`) | `gen_random_uuid()` | | `NEWID()` |
| Number | `col + 1`, `col - 1` | same | same | same |
| Text | `CONCAT(col, '')`, `UPPER`, `LOWER`, `TRIM` | `col \|\| ''`, `upper`, `lower`, `trim` | `col \|\| ''`, `upper`, `lower`, `trim` | `CONCAT(col, '')`, `UPPER`, `LOWER`, `LTRIM(RTRIM(col))` |
| Any | `NULL`, `DEFAULT` | `NULL`, `DEFAULT` | `NULL` | `NULL`, `DEFAULT` |

A staged expression shows in the cell as an italic `fx NOW()` until it is saved; after the save the row shows the value the database computed. Undo, redo and Revert treat it like any other edit, and a new row's cells can hold one too. An expression containing a comment, a `;` or a `,` outside parentheses is refused when you commit it, because it could change more than the cell (a comment could hide the `WHERE` of the `UPDATE`). Other databases, query results and document collections do not offer expressions.

### Review changes before saving

**Review** in the edit bar, next to Save and Revert, or `Ctrl+Alt+S` (`Cmd+Alt+S` on macOS), shows the statements Save will run for the table's unsaved changes, in the order it runs them: deletes, then new rows, then edited rows. They are built by the save itself and rendered by the connection's driver, so they are what the database receives, with SQL expressions unquoted. Values are written into the statements as literals; the MySQL, MariaDB, PostgreSQL, SQLite and SQL Server drivers send no bind parameters. A change that cannot be saved (a row without its key) shows as a comment saying why.

From the review, **Save** (or `Enter`) runs those statements, without the separate confirmation deletes otherwise ask for; **Copy SQL** copies them as a script; **Cancel** (or `Escape`) closes the review and keeps the changes. With **Review changes before saving** on in Settings > General > Data grid, every Save of a table grid opens the review first.

### Generated IDs

A cell can be given a new identifier generated by DBFlux and written as an ordinary value, so it works with every database: **UUID v4** (random), **UUID v7** (time-ordered), **KSUID** (27 characters, sortable by second) and **ULID** (26 characters, sortable by millisecond).

- The cell's context menu has a **Generate …** entry for each kind the column can store. On a cell of a larger selection, every selected cell gets its own new value, never a copy, and one undo takes them all back.
- For a column that looks like an identifier (a `uuid` type, a key, a text column sized for one of the kinds, or a name such as `user_id`), the list under the inline editor offers the kinds first, UUIDs on top; `Down` / `Up` write a new value into the editor. Committing it over a multi-cell selection again gives each cell its own value.
- The cell editor dialog shows a **Generate …** button per kind.

A `uuid` or `uniqueidentifier` column takes the UUIDs in their hyphenated form. A text column takes the kinds that fit its declared length: a `char(36)` the UUIDs, a `char(27)` a KSUID, a `varchar(30)` a KSUID or a ULID, and a column of unknown length all four. Binary columns such as MySQL's `binary(16)` are not offered generated IDs, because the grid writes text and they need the raw 16 bytes; use the `UNHEX(REPLACE(UUID(), '-', ''))` expression there instead.

### When results are editable

Plain table browses are editable when the table has a primary key. Results
produced by the **visual query builder** (SELECT mode) are also editable, but
only when they are provably bound to a single table: the result maps 1:1 to one
underlying table and every primary-key column of that table is projected under
its original name. Edits and deletes then build their `WHERE` from the projected
primary-key values.

JOINs are allowed: columns from the source table are editable, while joined
columns are read-only.

A builder result falls back to **read-only** — with a toolbar hint explaining why
— when any of these hold:

- The query aggregates or uses `GROUP BY` / `HAVING`.
- The projection is a wildcard across a JOIN.
- A primary-key column is missing or projected under an alias.
- The table's keys have not been loaded from the schema cache yet (the grid
  upgrades itself to editable once the keys arrive).

Free-form SQL typed into the editor stays read-only; inline edit applies only to
plain table browses and builder-generated SELECTs.

### Aggregated results

When a result comes from a grouped (`GROUP BY`) query, rows show the aggregated
output and editing is disabled — add-row, delete-row, edit-cell, and inspect-row
are unavailable, with explanatory tooltips. Pagination counts the grouped rows
(not the underlying rows), so the page total is accurate. Aggregate columns keep
the correct column kind, so charting still works.

## Copy as CSV or JSON

The context menu of a table result has **Copy as CSV** and **Copy as JSON**
below **Copy**. They copy the selection, whether one range or several picked
with `Ctrl`+click, through the same exporter as **Copy loaded rows to
clipboard**: CSV starts with a header row of the column names, and JSON is an
array with one object per row, keyed by column name, with numbers, booleans and
`null` kept as such. Every row and column the selection covers is included; a
cell inside that area but outside the selection is empty in CSV and `null` in
JSON. The copies follow what the grid shows: an unsaved edit is copied with
its new value and an added row that is not saved yet at its place. In JSON,
columns that share a name (`a.id` and `b.id` of a join) share one key, which
takes the value of the last of them. Plain **Copy** (`Ctrl+c`) copies
tab-separated text, also with unsaved edits and added rows, without a header
unless **Include column names when copying** is on in Settings → General →
Data grid, which starts it with a line of the copied columns' names.
`Ctrl+Alt+c` (`Cmd+Alt+c` on macOS) runs **Copy as JSON** from the keyboard;
the menu shows it next to the item.

Below them the menu has three more extractors:

- **Copy as SQL IN list** copies the selected values of one column as a list
  for a `WHERE ... IN`, such as `('a', 'O''Brien')` or `(1, 2, 3)`, quoted the
  way the connection's driver writes literals (ANSI quoting for a result
  without a connection). A value selected more than once is listed once and
  `NULL` is left out, since `IN (NULL)` matches nothing; a selection of NULLs
  alone copies nothing and says so. With several columns selected it lists the
  leftmost one. A collection's menu does not offer it.
- **Copy as INSERT statements**, on a table, copies one `INSERT` per selected
  row with the selected columns, written by the same driver generator as
  **Copy as Query**. Each row gives its own values for those columns, even
  where a cell inside the selected area is not itself selected, and a row
  holding a value the driver cannot write is left out. It is not offered on
  grouped rows or on a query builder join, whose columns are not all the
  table's.
- **Copy as Markdown table** copies the same rows and columns as Copy as CSV as
  a Markdown table with a header row; `NULL` and unselected cells are empty,
  bytes are written as `\x` hex and arrays and documents as JSON, and `|`,
  `&`, `<` and line breaks inside values are escaped.

All three copy what the grid shows, as Copy as CSV does: an unsaved edit with
its new value and a row added in the grid and not saved yet at its place.

## Copy as Query

The result context menu includes **Copy as Query**, which generates a
driver-specific mutation statement (or envelope, for non-SQL drivers) from the
selected row using the driver's own query generator.

## Exporting

Press `Ctrl+e` (`Cmd+e`) in the results panel, or run **Export results** from the
command palette. The available formats depend on the result shape and include:

- **CSV**
- **JSON (pretty)** and **JSON (compact)**
- **Text**
- **Binary** (for binary-shaped results)

The top of the menu chooses what is exported, and the **Save as file** and
**Copy to clipboard** formats below it follow that choice:

- **Rows**: **Selected** writes the rows that hold a selected cell as the grid
  shows them, unsaved edits and added rows included, as Copy as CSV does;
  **Loaded** the rows the grid holds (for a table, the current page), and
  **All** every row of the table or collection (see below; offered only where
  the grid can read it again page by page). **Selected** is disabled, with the
  reason under the row, until a cell is selected.
- **Columns**: **Visible**, the default, writes the shown columns in the order
  the grid shows them, so hidden columns are left out and moved or pinned
  columns keep their place. **All** writes every column in result order, hidden
  ones included. **Selected** writes the shown columns that hold a selected
  cell, in screen order. Selected rows with selected columns write the cells of
  a selected range with their values; with several `Ctrl`+click ranges, every
  row and column they cover.
- **Include header row (column names)**, on by default, and **Delimiter**
  (comma, semicolon or tab) apply to CSV saved or copied from this menu and to
  Export all rows as CSV. The context menu's Copy as CSV is not affected.

The caption over the options states what the formats write, for example
"Export 12 selected rows, 4 columns", and the toast after a save or copy states
the same rows and columns with the path or the byte count. The menu opens on the
first format, so `Ctrl+e` `Enter` still saves at once. `k` or `Up` moves up to
the options; `Enter`, `Right` or `l` on an option row steps to its next choice,
passing over the ones that do not apply, or ticks the header checkbox, and a
click picks a choice directly. Your choices are kept for every grid, not per
table, and across restarts.

With **Rows** set to **All**, the menu offers **Export all rows to file**, in
CSV or in JSON with one object per line. It reads the whole table or collection
again with the grid's current filter, sort and column selection, one page at a
time, writes the columns the **Columns** option picks, and streams it into the
one file you choose, so the data never has to fit in memory. A table shown through the query
builder or a relational filter is read through the same query, starting at its
offset and stopping at its limit when it sets one. The export runs as a task in
the Tasks panel, which shows its progress and can cancel it; the toast states
the total row count and the path.

A document collection exported as JSON writes each document whole, one object
per line, with every field it has. As CSV it is written with the columns the
grid shows for the first page, including expanded objects. Documents of one
collection need not share their fields, so in a CSV file a field that first
appears on a later page has no column, because the header is already written;
the export then names those fields in a warning.

A query result offers only the loaded rows, because exporting all of them would
mean running the query again. So does a nearest-neighbor search: it returns a
fixed number of the closest rows, and the grid already holds all of them.
