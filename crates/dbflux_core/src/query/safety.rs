use dbflux_policy::ExecutionClassification;

use crate::QueryLanguage;

use super::language_service::{LanguageService, classify_query_for_language_with_service};

/// Lexical rules of one SQL dialect. Classification lexes the query once per
/// dialect and keeps the most restrictive result, so a keyword counts as code
/// when any supported dialect would read it as code. This closes gaps where
/// one dialect sees a string or comment that another dialect executes.
#[derive(Clone, Copy, Debug)]
struct LexRules {
    nested_block_comments: bool,
    dash_comment_requires_space: bool,
    hash_line_comments: bool,
    executable_comments: bool,
    backslash_escapes: bool,
    escape_string_prefix: bool,
    dollar_quotes: bool,
    bracket_identifiers: bool,
}

const POSTGRES_RULES: LexRules = LexRules {
    nested_block_comments: true,
    dash_comment_requires_space: false,
    hash_line_comments: false,
    executable_comments: false,
    backslash_escapes: false,
    escape_string_prefix: true,
    dollar_quotes: true,
    bracket_identifiers: false,
};

const MYSQL_RULES: LexRules = LexRules {
    nested_block_comments: false,
    dash_comment_requires_space: true,
    hash_line_comments: true,
    executable_comments: true,
    backslash_escapes: true,
    escape_string_prefix: false,
    dollar_quotes: false,
    bracket_identifiers: false,
};

const SQLITE_RULES: LexRules = LexRules {
    nested_block_comments: false,
    dash_comment_requires_space: false,
    hash_line_comments: false,
    executable_comments: false,
    backslash_escapes: false,
    escape_string_prefix: false,
    dollar_quotes: false,
    bracket_identifiers: true,
};

const TSQL_RULES: LexRules = LexRules {
    nested_block_comments: true,
    dash_comment_requires_space: false,
    hash_line_comments: false,
    executable_comments: false,
    backslash_escapes: false,
    escape_string_prefix: false,
    dollar_quotes: false,
    bracket_identifiers: true,
};

const DIALECT_RULES: [LexRules; 4] = [POSTGRES_RULES, MYSQL_RULES, SQLITE_RULES, TSQL_RULES];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanState {
    Normal,
    LineComment,
    BlockComment {
        depth: usize,
    },
    Quoted {
        close: char,
        backslash_escapes: bool,
    },
    DollarQuoted {
        tag_start: usize,
        tag_len: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenKind {
    /// An unquoted word, uppercased.
    Word(String),
    /// A string literal or quoted identifier; its content is never code.
    Quoted,
    Symbol(char),
}

#[derive(Clone, Debug)]
struct Token {
    kind: TokenKind,
}

impl Token {
    fn word(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Word(word) => Some(word),
            _ => None,
        }
    }

    fn is_symbol(&self, symbol: char) -> bool {
        self.kind == TokenKind::Symbol(symbol)
    }
}

/// Classify a SQL query's execution impact for MCP/policy governance and for
/// editor auto-refresh.
///
/// This is a lexical heuristic, not a security boundary on its own. It finds
/// writes hidden in data-modifying CTEs, `EXPLAIN ANALYZE`, `INTO` clauses,
/// locking reads, multi-statement batches and calls to the built-in functions
/// in [`side_effecting_function`]. It cannot see what a user-defined function
/// does, so a database role without write privileges stays the real boundary.
pub fn classify_sql_execution(sql: &str) -> ExecutionClassification {
    DIALECT_RULES
        .iter()
        .map(|rules| classify_with_rules(sql, rules))
        .fold(
            ExecutionClassification::Metadata,
            ExecutionClassification::max,
        )
}

/// Classify a query's execution impact for MCP/policy governance.
///
/// Pass the live connection's `Connection::language_service()` as `service`
/// whenever one is available, so driver-owned dialects (`InfluxQuery`, `Flux`,
/// `Custom(_)`, `MongoQuery`, and the CloudWatch/OpenSearch trio) are
/// classified by the driver instead of a conservative default. `None`
/// preserves the language-keyed fallback classification for those dialects —
/// for `MongoQuery` that fallback is the pre-existing core text heuristic
/// (`classify_mongo_query`), unchanged. `Sql` and `RedisCommands` always use
/// the shared core heuristic regardless of `service`.
pub fn classify_query_for_governance(
    query_language: &QueryLanguage,
    query: &str,
    service: Option<&dyn LanguageService>,
) -> ExecutionClassification {
    classify_query_for_language_with_service(query_language, query, service)
}

pub fn is_safe_read_query(sql: &str) -> bool {
    let has_statement_in_every_dialect = DIALECT_RULES
        .iter()
        .all(|rules| !tokenize(sql, rules).is_empty());

    has_statement_in_every_dialect
        && matches!(
            classify_sql_execution(sql),
            ExecutionClassification::Read | ExecutionClassification::Metadata
        )
}

/// Whether `sql` holds a compound block: the `BEGIN ... END` body of a
/// procedure, function or trigger, an anonymous block, `BEGIN TRY` or
/// `BEGIN ATOMIC`. The statement splitter cuts such a block at its inner `;`,
/// so its pieces are not standalone statements.
///
/// A `BEGIN` that opens a transaction is not a block, nor one used as a
/// column name (MySQL allows `begin` unquoted). A block counts only when every
/// dialect sees it outside comments, strings and quoted bodies, so a
/// PostgreSQL `DO $$ BEGIN ... END $$` body is not one.
pub fn contains_compound_block(sql: &str) -> bool {
    DIALECT_RULES.iter().all(|rules| {
        let tokens = tokenize(sql, rules);
        tokens.iter().enumerate().any(|(index, token)| {
            let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
            let next = tokens.get(index + 1);

            token.word() == Some("BEGIN")
                && !begin_opens_transaction(next)
                && !begin_is_identifier(previous, next)
        })
    })
}

/// Whether a `BEGIN` between `previous` and `next` is a column or table name:
/// qualified (`t.begin`), followed by a symbol (`begin,`, `begin = 1`), or
/// followed by a word that continues an expression or clause. Anything else
/// is taken as a block, which only makes the editor run the whole buffer.
fn begin_is_identifier(previous: Option<&Token>, next: Option<&Token>) -> bool {
    if previous.is_some_and(|token| token.is_symbol('.')) {
        return true;
    }

    match next {
        Some(Token {
            kind: TokenKind::Symbol(symbol),
        }) => *symbol != ';',
        Some(token) => matches!(
            token.word(),
            Some(
                "FROM"
                    | "AS"
                    | "WHERE"
                    | "AND"
                    | "OR"
                    | "IS"
                    | "IN"
                    | "LIKE"
                    | "BETWEEN"
                    | "ORDER"
                    | "GROUP"
                    | "HAVING"
                    | "LIMIT"
                    | "ON"
                    | "JOIN"
                    | "INTO"
                    | "UNION"
                    | "ASC"
                    | "DESC"
                    | "THEN"
                    | "ELSE"
                    | "END"
            )
        ),
        None => false,
    }
}

/// Whether a `BEGIN` followed by `next` opens a transaction (`BEGIN;`,
/// `BEGIN TRANSACTION`, `BEGIN ISOLATION LEVEL ...`, SQLite's
/// `BEGIN IMMEDIATE`) rather than a compound block.
fn begin_opens_transaction(next: Option<&Token>) -> bool {
    match next {
        None => true,
        Some(token) if token.is_symbol(';') => true,
        Some(token) => matches!(
            token.word(),
            Some(
                "TRANSACTION"
                    | "TRAN"
                    | "WORK"
                    | "ISOLATION"
                    | "READ"
                    | "DEFERRED"
                    | "IMMEDIATE"
                    | "EXCLUSIVE"
                    | "DISTRIBUTED"
            )
        ),
    }
}

fn classify_with_rules(sql: &str, rules: &LexRules) -> ExecutionClassification {
    let tokens = tokenize(sql, rules);

    let statements: Vec<&[Token]> = tokens
        .split(|token| token.is_symbol(';'))
        .filter(|statement| !statement.is_empty())
        .collect();

    let classification = statements
        .iter()
        .map(|statement| classify_statement(statement))
        .fold(
            ExecutionClassification::Metadata,
            ExecutionClassification::max,
        );

    if statements.len() > 1 {
        classification.max(ExecutionClassification::Write)
    } else {
        classification
    }
}

fn classify_statement(tokens: &[Token]) -> ExecutionClassification {
    let Some((first, rest)) = skip_leading_symbols(tokens).split_first() else {
        return ExecutionClassification::Write;
    };

    let Some(keyword) = first.word() else {
        return ExecutionClassification::Write;
    };

    match keyword {
        "EXPLAIN" | "DESC" | "DESCRIBE" => classify_explain(rest),
        "SHOW" => ExecutionClassification::Metadata,
        "SELECT" | "WITH" => classify_read_body(rest),
        "EXEC" | "EXECUTE" => classify_procedure_call(rest),
        _ => classify_leading_keyword(keyword),
    }
}

/// SQL Server's extended procedures (`xp_cmdshell`, `xp_regwrite`, ...) and
/// its server configuration procedures act on the host or the instance, not
/// on data.
fn classify_procedure_call(tokens: &[Token]) -> ExecutionClassification {
    let procedure = tokens
        .iter()
        .take_while(|token| token.word().is_some() || token.is_symbol('.'))
        .filter_map(Token::word)
        .last();

    match procedure {
        Some(name)
            if name.starts_with("XP_")
                || matches!(
                    name,
                    "SP_CONFIGURE"
                        | "SP_ADDLOGIN"
                        | "SP_ADDSRVROLEMEMBER"
                        | "SP_ADDROLEMEMBER"
                        | "SP_PASSWORD"
                        | "SP_ADDLINKEDSERVER"
                        | "SP_ADDEXTENDEDPROC"
                        | "SP_OACREATE"
                ) =>
        {
            ExecutionClassification::Admin
        }
        _ => ExecutionClassification::Write,
    }
}

/// The class of a call to a built-in function that changes state even though
/// it appears in a `SELECT`: it ends other sessions, changes settings or
/// sequences, writes through another connection, takes locks, or reads and
/// writes files on the database server. `name` is uppercased.
///
/// Read-only transactions do not stop most of these (PostgreSQL still runs
/// `pg_terminate_backend` and `dblink_exec` inside `BEGIN READ ONLY`), so the
/// classifier has to.
fn side_effecting_function(name: &str) -> Option<ExecutionClassification> {
    use ExecutionClassification::{Admin, Write};

    let class = match name {
        // PostgreSQL: sessions, server and configuration control.
        "PG_TERMINATE_BACKEND"
        | "PG_CANCEL_BACKEND"
        | "PG_RELOAD_CONF"
        | "PG_ROTATE_LOGFILE"
        | "PG_PROMOTE"
        | "PG_SWITCH_WAL"
        | "PG_SWITCH_XLOG"
        | "PG_CREATE_RESTORE_POINT"
        | "PG_BACKUP_START"
        | "PG_BACKUP_STOP"
        | "PG_START_BACKUP"
        | "PG_STOP_BACKUP"
        | "PG_LOG_BACKEND_MEMORY_CONTEXTS"
        | "PG_IMPORT_SYSTEM_COLLATIONS"
        | "SET_CONFIG" => Admin,
        // PostgreSQL: server files, including large object import and export.
        "PG_STAT_FILE" | "LO_IMPORT" | "LO_EXPORT" => Admin,
        // PostgreSQL: sequences, notifications, large objects, logical
        // decoding, and SQL text that another function executes.
        "NEXTVAL"
        | "SETVAL"
        | "PG_NOTIFY"
        | "LO_CREATE"
        | "LO_CREAT"
        | "LO_UNLINK"
        | "LO_FROM_BYTEA"
        | "LO_PUT"
        | "LOWRITE"
        | "LO_TRUNCATE"
        | "LO_TRUNCATE64"
        | "PG_LOGICAL_EMIT_MESSAGE"
        | "PG_LOGICAL_SLOT_GET_CHANGES"
        | "PG_LOGICAL_SLOT_GET_BINARY_CHANGES"
        | "QUERY_TO_XML"
        | "QUERY_TO_XMLSCHEMA"
        | "QUERY_TO_XML_AND_XMLSCHEMA" => Write,
        // MySQL / MariaDB: named locks, server files, command UDFs.
        "GET_LOCK" | "RELEASE_LOCK" | "RELEASE_ALL_LOCKS" => Write,
        "LOAD_FILE" | "SYS_EXEC" | "SYS_EVAL" => Admin,
        // SQLite: extensions and the CLI's file functions.
        "LOAD_EXTENSION" | "READFILE" | "WRITEFILE" => Admin,
        // SQL Server: ad hoc remote and file access, pass-through queries.
        "OPENROWSET" | "OPENDATASOURCE" => Admin,
        "OPENQUERY" => Write,
        _ if name.starts_with("PG_READ_")
            || name.starts_with("PG_LS_")
            || name.starts_with("PG_FILE_")
            || name.starts_with("PG_STAT_RESET")
            || name.starts_with("PG_WAL_REPLAY_")
            || name.starts_with("PG_REPLICATION_ORIGIN_")
            || name.contains("REPLICATION_SLOT")
            || name.starts_with("XP_") =>
        {
            Admin
        }
        _ if name.starts_with("DBLINK")
            || name.starts_with("PG_ADVISORY_")
            || name.starts_with("PG_TRY_ADVISORY_") =>
        {
            Write
        }
        _ => return None,
    };

    Some(class)
}

/// Drops the symbols, such as opening parentheses, that precede a statement's
/// first word or quoted token.
fn skip_leading_symbols(tokens: &[Token]) -> &[Token] {
    let first_index = tokens
        .iter()
        .position(|token| !matches!(token.kind, TokenKind::Symbol(_)))
        .unwrap_or(tokens.len());

    tokens.get(first_index..).unwrap_or_default()
}

fn classify_leading_keyword(keyword: &str) -> ExecutionClassification {
    match keyword {
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" => ExecutionClassification::Write,
        "TRUNCATE" | "DROP" | "ALTER" => ExecutionClassification::Destructive,
        "GRANT" | "REVOKE" | "CREATE" | "SET" | "SYSTEM" | "KILL" | "ATTACH" | "OPTIMIZE"
        | "BACKUP" | "RESTORE" | "UNDROP" | "MOVE" | "INSTALL" | "LOAD" => {
            ExecutionClassification::Admin
        }
        "DETACH" | "RENAME" | "EXCHANGE" => ExecutionClassification::Destructive,
        _ => ExecutionClassification::Write,
    }
}

/// `EXPLAIN` only plans a statement, unless `ANALYZE` makes it execute the
/// statement, in which case the explained statement decides the class.
///
/// A quoted name in the parenthesized option list counts as `ANALYZE`, because
/// PostgreSQL accepts `EXPLAIN ("analyze") ...`.
fn classify_explain(tokens: &[Token]) -> ExecutionClassification {
    let mut analyze = false;
    let mut index = 0;

    if tokens.first().is_some_and(|token| token.is_symbol('(')) {
        let mut depth = 0usize;

        while let Some(token) = tokens.get(index) {
            index += 1;

            if token.is_symbol('(') {
                depth += 1;
            } else if token.is_symbol(')') {
                depth -= 1;

                if depth == 0 {
                    break;
                }
            } else if matches!(token.word(), Some("ANALYZE" | "ANALYSE"))
                || token.kind == TokenKind::Quoted
            {
                analyze = true;
            }
        }
    }

    while let Some(token) = tokens.get(index) {
        let is_option = match &token.kind {
            TokenKind::Word(word) => matches!(
                word.as_str(),
                "ANALYZE"
                    | "ANALYSE"
                    | "VERBOSE"
                    | "QUERY"
                    | "PLAN"
                    | "EXTENDED"
                    | "PARTITIONS"
                    | "FORMAT"
                    | "TREE"
                    | "JSON"
                    | "TRADITIONAL"
            ),
            TokenKind::Symbol('=') => true,
            _ => false,
        };

        if !is_option {
            break;
        }

        if matches!(token.word(), Some("ANALYZE" | "ANALYSE")) {
            analyze = true;
        }

        index += 1;
    }

    let target = tokens.get(index..).unwrap_or_default();

    // No dialect explains an EXPLAIN; refusing it also bounds the recursion to
    // one level, even when the nested EXPLAIN sits inside parentheses. It is
    // checked before ANALYZE, because the nested one may carry it
    // (`EXPLAIN EXPLAIN ANALYZE DELETE ...`).
    let explains_an_explain = skip_leading_symbols(target)
        .first()
        .and_then(Token::word)
        .is_some_and(|word| matches!(word, "EXPLAIN" | "DESC" | "DESCRIBE"));

    if explains_an_explain {
        return ExecutionClassification::Write;
    }

    if !analyze || target.is_empty() {
        return ExecutionClassification::Metadata;
    }

    classify_statement(target)
}

/// Classifies the body of a `SELECT` or `WITH` statement: a write keyword
/// anywhere (for example a data-modifying CTE), an `INTO` clause or a locking
/// clause makes it a write. An unquoted identifier spelled like a write
/// keyword is over-classified on purpose.
fn classify_read_body(tokens: &[Token]) -> ExecutionClassification {
    let mut classification = ExecutionClassification::Read;

    for (index, token) in tokens.iter().enumerate() {
        let previous = index
            .checked_sub(1)
            .and_then(|previous| tokens.get(previous));
        let next = tokens.get(index + 1);
        let is_function_call = next.is_some_and(|next| next.is_symbol('('));

        // A quoted name can call any function (`"pg_terminate_backend"(1)`)
        // and its text is not kept, so the call counts as a write.
        if token.kind == TokenKind::Quoted && is_function_call {
            classification = classification.max(ExecutionClassification::Write);
            continue;
        }

        let Some(word) = token.word() else {
            continue;
        };

        // Checked before the qualified-name skip, so `pg_catalog.setval(...)`
        // counts like `setval(...)`.
        if is_function_call && let Some(found) = side_effecting_function(word) {
            classification = classification.max(found);
            continue;
        }

        let qualifier = index
            .checked_sub(2)
            .and_then(|qualifier| tokens.get(qualifier));

        // `1.` is a numeric literal, so a word after it is not a qualified name.
        let is_qualified_name = previous.is_some_and(|previous| previous.is_symbol('.'))
            && qualifier.is_some_and(|qualifier| match &qualifier.kind {
                TokenKind::Word(word) => {
                    !word.starts_with(|character: char| character.is_ascii_digit())
                }
                TokenKind::Quoted => true,
                TokenKind::Symbol(_) => false,
            });
        let is_lone_bracketed_name = previous.is_some_and(|previous| previous.is_symbol('['))
            && next.is_some_and(|next| next.is_symbol(']'));

        if is_qualified_name || is_lone_bracketed_name {
            continue;
        }

        let found = match word {
            "REPLACE" | "INSERT" | "TRUNCATE" if is_function_call => None,
            "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "UPSERT" | "COPY" | "CALL"
            | "DO" | "LOCK" | "INTO" => Some(ExecutionClassification::Write),
            "TRUNCATE" | "DROP" | "ALTER" => Some(ExecutionClassification::Destructive),
            "CREATE" | "GRANT" | "REVOKE" => Some(ExecutionClassification::Admin),
            "FOR"
                if next
                    .and_then(Token::word)
                    .is_some_and(|next| matches!(next, "UPDATE" | "SHARE" | "NO" | "KEY")) =>
            {
                Some(ExecutionClassification::Write)
            }
            _ => None,
        };

        if let Some(found) = found {
            classification = classification.max(found);
        }
    }

    classification
}

fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$'
}

/// Length in chars of a dollar-quote tag (`$$` or `$tag$`) starting at `start`.
fn dollar_tag_len(chars: &[(usize, char)], start: usize) -> Option<usize> {
    let mut index = start + 1;

    match chars.get(index).map(|&(_, character)| character) {
        Some('$') => return Some(2),
        Some(character) if character.is_alphabetic() || character == '_' => {}
        _ => return None,
    }

    while let Some(&(_, character)) = chars.get(index) {
        if character == '$' {
            return Some(index - start + 1);
        }

        if !(character.is_alphanumeric() || character == '_') {
            return None;
        }

        index += 1;
    }

    None
}

/// Splits `sql` into words and symbols outside comments, string literals and
/// quoted identifiers, following one dialect's lexical rules.
fn tokenize(sql: &str, rules: &LexRules) -> Vec<Token> {
    let chars: Vec<(usize, char)> = sql.char_indices().collect();
    let char_at = |index: usize| chars.get(index).map(|&(_, character)| character);

    let mut tokens = Vec::new();
    let mut state = ScanState::Normal;
    let mut index = 0;

    while let Some(&(_, current)) = chars.get(index) {
        let next = char_at(index + 1);

        match state {
            ScanState::Normal => {
                let previous_is_identifier = index
                    .checked_sub(1)
                    .and_then(char_at)
                    .is_some_and(is_identifier_char);

                if current.is_whitespace() {
                    index += 1;
                    continue;
                }

                if current == '-' && next == Some('-') {
                    let after = char_at(index + 2);
                    let starts_comment = !rules.dash_comment_requires_space
                        || after.is_none_or(|after| after.is_whitespace() || after.is_control());

                    if starts_comment {
                        state = ScanState::LineComment;
                        index += 2;
                        continue;
                    }
                }

                if current == '#' && rules.hash_line_comments {
                    state = ScanState::LineComment;
                    index += 1;
                    continue;
                }

                if current == '/' && next == Some('*') {
                    // MySQL executes the body of `/*! ... */` and MariaDB also that of
                    // `/*M! ... */`, so it is scanned as code.
                    let executable_prefix_len = match (char_at(index + 2), char_at(index + 3)) {
                        (Some('!'), _) => Some(3),
                        (Some('M' | 'm'), Some('!')) => Some(4),
                        _ => None,
                    };

                    if rules.executable_comments
                        && let Some(prefix_len) = executable_prefix_len
                    {
                        index += prefix_len;

                        while char_at(index).is_some_and(|character| character.is_ascii_digit()) {
                            index += 1;
                        }

                        continue;
                    }

                    state = ScanState::BlockComment { depth: 1 };
                    index += 2;
                    continue;
                }

                let close = match current {
                    '\'' | '"' | '`' => Some(current),
                    '[' if rules.bracket_identifiers => Some(']'),
                    _ => None,
                };

                if let Some(close) = close {
                    let escape_prefixed = current == '\''
                        && rules.escape_string_prefix
                        && index
                            .checked_sub(1)
                            .and_then(char_at)
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&'e'))
                        && !index
                            .checked_sub(2)
                            .and_then(char_at)
                            .is_some_and(is_identifier_char);

                    let backslash_escapes = matches!(current, '\'' | '"')
                        && (rules.backslash_escapes || escape_prefixed);

                    tokens.push(Token {
                        kind: TokenKind::Quoted,
                    });
                    state = ScanState::Quoted {
                        close,
                        backslash_escapes,
                    };
                    index += 1;
                    continue;
                }

                if current == '$'
                    && rules.dollar_quotes
                    && !previous_is_identifier
                    && let Some(tag_len) = dollar_tag_len(&chars, index)
                {
                    tokens.push(Token {
                        kind: TokenKind::Quoted,
                    });
                    state = ScanState::DollarQuoted {
                        tag_start: index,
                        tag_len,
                    };
                    index += tag_len;
                    continue;
                }

                let starts_word = current.is_alphanumeric()
                    || current == '_'
                    || (current == '$' && !rules.dollar_quotes);

                if starts_word {
                    let mut word = String::new();

                    while let Some(character) = char_at(index).filter(|&c| is_identifier_char(c)) {
                        word.push(character.to_ascii_uppercase());
                        index += 1;
                    }

                    tokens.push(Token {
                        kind: TokenKind::Word(word),
                    });
                    continue;
                }

                tokens.push(Token {
                    kind: TokenKind::Symbol(current),
                });
                index += 1;
            }

            ScanState::LineComment => {
                if current == '\n' {
                    state = ScanState::Normal;
                }

                index += 1;
            }

            ScanState::BlockComment { depth } => {
                if current == '*' && next == Some('/') {
                    state = if depth == 1 {
                        ScanState::Normal
                    } else {
                        ScanState::BlockComment { depth: depth - 1 }
                    };
                    index += 2;
                } else if rules.nested_block_comments && current == '/' && next == Some('*') {
                    state = ScanState::BlockComment { depth: depth + 1 };
                    index += 2;
                } else {
                    index += 1;
                }
            }

            ScanState::Quoted {
                close,
                backslash_escapes,
            } => {
                if backslash_escapes && current == '\\' {
                    index += 2;
                    continue;
                }

                if current == close {
                    if next == Some(close) {
                        index += 2;
                        continue;
                    }

                    state = ScanState::Normal;
                }

                index += 1;
            }

            ScanState::DollarQuoted { tag_start, tag_len } => {
                let closes =
                    (0..tag_len).all(|step| char_at(index + step) == char_at(tag_start + step));

                if closes {
                    state = ScanState::Normal;
                    index += tag_len;
                } else {
                    index += 1;
                }
            }
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use dbflux_policy::ExecutionClassification;

    use crate::QueryLanguage;

    use super::{
        classify_query_for_governance, classify_sql_execution, contains_compound_block,
        is_safe_read_query,
    };

    #[test]
    fn allows_basic_read_queries() {
        assert!(is_safe_read_query("SELECT * FROM users"));
        assert!(is_safe_read_query(
            "with cte as (select 1) select * from cte"
        ));
        assert!(is_safe_read_query("SHOW TABLES"));
        assert!(is_safe_read_query("DESC users"));
    }

    #[test]
    fn rejects_write_queries() {
        assert!(!is_safe_read_query("INSERT INTO users VALUES (1)"));
        assert!(!is_safe_read_query("UPDATE users SET name = 'a'"));
        assert!(!is_safe_read_query("DELETE FROM users"));
        assert!(!is_safe_read_query("DROP TABLE users"));
    }

    #[test]
    fn rejects_multiple_statements() {
        assert!(!is_safe_read_query("SELECT 1; DROP TABLE users"));
        assert!(!is_safe_read_query("SELECT 1; SELECT 2"));
    }

    #[test]
    fn allows_single_statement_with_trailing_semicolon() {
        assert!(is_safe_read_query("SELECT 1;"));
        assert!(is_safe_read_query("-- comment\nSELECT 1;"));
    }

    #[test]
    fn strips_comments_before_keyword_detection() {
        assert!(is_safe_read_query("-- hello\nSELECT * FROM users"));
        assert!(is_safe_read_query("/* hello */ SELECT * FROM users"));
        assert!(!is_safe_read_query("/* hello */ DELETE FROM users"));
    }

    #[test]
    fn sql_classification_maps_read_write_and_destructive_classes() {
        assert_eq!(
            classify_sql_execution("SELECT * FROM users"),
            ExecutionClassification::Read
        );
        assert_eq!(
            classify_sql_execution("EXPLAIN SELECT * FROM users"),
            ExecutionClassification::Metadata
        );
        assert_eq!(
            classify_sql_execution("UPDATE users SET active = true"),
            ExecutionClassification::Write
        );
        assert_eq!(
            classify_sql_execution("DROP TABLE users"),
            ExecutionClassification::Destructive
        );
    }

    #[test]
    fn ambiguous_query_escalates_conservatively() {
        assert_eq!(
            classify_query_for_governance(&QueryLanguage::Sql, "VACUUM users", None),
            ExecutionClassification::Write
        );
    }

    #[test]
    fn language_service_delegation_overrides_the_conservative_default() {
        use super::super::language_service::{
            DangerousQueryKind, LanguageService, ValidationResult,
        };

        struct FakeService;

        impl LanguageService for FakeService {
            fn validate(&self, _query: &str) -> ValidationResult {
                ValidationResult::Valid
            }

            fn detect_dangerous(&self, _query: &str) -> Option<DangerousQueryKind> {
                None
            }

            fn classify_execution(&self, _query: &str) -> Option<ExecutionClassification> {
                Some(ExecutionClassification::Destructive)
            }
        }

        let service = FakeService;

        assert_eq!(
            classify_query_for_governance(
                &QueryLanguage::InfluxQuery,
                "DROP DATABASE x",
                Some(&service)
            ),
            ExecutionClassification::Destructive
        );

        assert_eq!(
            classify_query_for_governance(&QueryLanguage::InfluxQuery, "DROP DATABASE x", None),
            ExecutionClassification::Write
        );
    }

    fn assert_classified(queries: &[&str], expected: ExecutionClassification) {
        for query in queries {
            assert_eq!(
                classify_sql_execution(query),
                expected,
                "unexpected classification for: {query}"
            );
            assert!(
                !is_safe_read_query(query),
                "must not be a safe read: {query}"
            );
        }
    }

    fn assert_safe_reads(queries: &[&str]) {
        for query in queries {
            assert_eq!(
                classify_sql_execution(query),
                ExecutionClassification::Read,
                "unexpected classification for: {query}"
            );
            assert!(is_safe_read_query(query), "must be a safe read: {query}");
        }
    }

    #[test]
    fn data_modifying_statements_inside_a_cte_are_writes() {
        assert_classified(
            &[
                "WITH d AS (DELETE FROM t WHERE id = 1 RETURNING *) SELECT * FROM d",
                "with i as (insert into t (id) values (1) returning id) select id from i",
                "WITH u AS (UPDATE t SET a = 1 RETURNING *) SELECT * FROM u",
                "WITH m AS (MERGE INTO t USING s ON t.id = s.id \
                 WHEN MATCHED THEN DELETE RETURNING *) SELECT * FROM m",
                "WITH x AS (SELECT 1) , d AS (DELETE FROM t RETURNING 1) SELECT * FROM x",
            ],
            ExecutionClassification::Write,
        );
    }

    #[test]
    fn explain_analyze_is_classified_by_the_explained_statement() {
        assert_classified(
            &[
                "EXPLAIN ANALYZE DELETE FROM t WHERE id = 1",
                "explain analyse update t set a = 1",
                "EXPLAIN (ANALYZE) INSERT INTO t VALUES (1)",
                "EXPLAIN (ANALYZE true, BUFFERS) DELETE FROM t",
                "EXPLAIN (FORMAT JSON, ANALYZE) DELETE FROM t",
                "EXPLAIN ANALYZE VERBOSE WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
                "EXPLAIN ANALYZE FORMAT=TREE UPDATE t SET a = 1",
                "DESCRIBE ANALYZE UPDATE t SET a = 1",
                "EXPLAIN ANALYZE EXPLAIN ANALYZE SELECT 1",
                // PostgreSQL accepts a quoted identifier as an option name.
                "EXPLAIN (\"analyze\") DELETE FROM t",
                "EXPLAIN (FORMAT JSON, \"analyze\" true) DELETE FROM t",
                "EXPLAIN ANALYZE (EXPLAIN ANALYZE (SELECT 1))",
                "EXPLAIN (ANALYZE) (EXPLAIN (ANALYZE) SELECT 1)",
            ],
            ExecutionClassification::Write,
        );

        assert_eq!(
            classify_sql_execution("EXPLAIN ANALYZE SELECT * FROM t"),
            ExecutionClassification::Read
        );
        assert!(is_safe_read_query("EXPLAIN ANALYZE SELECT * FROM t"));
    }

    #[test]
    fn deeply_nested_explain_analyze_is_refused_without_deep_recursion() {
        let query = "EXPLAIN ANALYZE (".repeat(50_000);

        assert_eq!(
            classify_sql_execution(&query),
            ExecutionClassification::Write
        );
    }

    #[test]
    fn explain_without_analyze_stays_metadata() {
        for query in [
            "EXPLAIN DELETE FROM t WHERE id = 1",
            "EXPLAIN (FORMAT JSON) SELECT * FROM t",
            "EXPLAIN SELECT analyze FROM t",
            "EXPLAIN QUERY PLAN SELECT * FROM t",
            "DESC users",
        ] {
            assert_eq!(
                classify_sql_execution(query),
                ExecutionClassification::Metadata,
                "unexpected classification for: {query}"
            );
            assert!(is_safe_read_query(query), "must be a safe read: {query}");
        }
    }

    #[test]
    fn select_into_is_a_write() {
        assert_classified(
            &[
                "SELECT * INTO new_table FROM t",
                "SELECT * INTO #tmp FROM t",
                "SELECT a, b INTO OUTFILE '/tmp/out.csv' FROM t",
                "SELECT a FROM t INTO DUMPFILE '/tmp/out.bin'",
                // `1.` is a numeric literal, not a qualifier.
                "SELECT 1. INTO t2",
            ],
            ExecutionClassification::Write,
        );
    }

    #[test]
    fn locking_reads_are_writes() {
        assert_classified(
            &[
                "SELECT * FROM t WHERE id = 1 FOR UPDATE",
                "SELECT * FROM t FOR UPDATE SKIP LOCKED",
                "SELECT * FROM t FOR SHARE",
                "SELECT * FROM t FOR NO KEY UPDATE",
                "SELECT * FROM t FOR KEY SHARE NOWAIT",
                "SELECT * FROM t LOCK IN SHARE MODE",
                "SELECT * FROM t WHERE id = 1. FOR SHARE",
            ],
            ExecutionClassification::Write,
        );
    }

    #[test]
    fn every_statement_of_a_batch_is_classified() {
        assert_eq!(
            classify_sql_execution("SELECT 1; DROP TABLE users"),
            ExecutionClassification::Destructive
        );
        assert_eq!(
            classify_sql_execution("SHOW TABLES; DELETE FROM users"),
            ExecutionClassification::Write
        );
        assert_eq!(
            classify_sql_execution("EXPLAIN SELECT 1; DELETE FROM users"),
            ExecutionClassification::Write
        );
        assert_eq!(
            classify_sql_execution("SELECT 1; SELECT 2"),
            ExecutionClassification::Write
        );
    }

    #[test]
    fn writes_hidden_by_dialect_specific_lexing_are_detected() {
        assert_classified(
            &[
                // MySQL executes the body of a `/*! ... */` comment.
                "SELECT a FROM t /*!50000 INTO OUTFILE '/tmp/x' */",
                // MariaDB also executes the body of `/*M! ... */`.
                "SELECT a FROM t /*M!100000 INTO OUTFILE '/tmp/x' */",
                "SELECT a FROM t /*M! INTO OUTFILE '/tmp/x' */",
                // MySQL reads `\'` as an escaped quote, so the string closes early.
                r"SELECT 'a\'' , b INTO OUTFILE '/tmp/x' FROM t",
                // PostgreSQL block comments nest.
                "SELECT 1 /* /* */ ' */ , 2 INTO t2 FROM t -- '",
                // MySQL needs whitespace after `--` to start a comment.
                "SELECT 1--1 INTO OUTFILE '/tmp/x'",
            ],
            ExecutionClassification::Write,
        );
    }

    #[test]
    fn compound_blocks_are_told_apart_from_transactions() {
        let procedure = "CREATE PROCEDURE purge_logs()\nBEGIN\n  DELETE FROM logs WHERE id < 10;\n  INSERT INTO audit (action) VALUES ('purge');\nEND;";
        assert!(contains_compound_block(procedure));
        assert!(contains_compound_block(
            "BEGIN TRY\n  SELECT 1;\nEND TRY\nBEGIN CATCH\n  SELECT 2;\nEND CATCH;"
        ));
        assert!(contains_compound_block("BEGIN NOT ATOMIC SELECT 1; END;"));

        for transaction in [
            "BEGIN; SELECT 1; COMMIT;",
            "BEGIN",
            "BEGIN TRANSACTION; SELECT 1; COMMIT;",
            "BEGIN TRAN; SELECT 1; COMMIT TRAN;",
            "BEGIN WORK; SELECT 1; COMMIT;",
            "BEGIN ISOLATION LEVEL SERIALIZABLE; SELECT 1; COMMIT;",
            "BEGIN IMMEDIATE; SELECT 1; COMMIT;",
        ] {
            assert!(!contains_compound_block(transaction), "{transaction}");
        }

        assert!(!contains_compound_block(
            "DO $$ BEGIN PERFORM 1; END $$; SELECT 1;"
        ));
        assert!(!contains_compound_block("SELECT 'BEGIN x'; SELECT 1;"));
        assert!(contains_compound_block(
            "CREATE PROCEDURE p AS BEGIN SELECT 1; SELECT 2; END;"
        ));
        assert!(contains_compound_block(
            "IF @x = 1 BEGIN SELECT 1; SELECT 2; END"
        ));

        for identifier in [
            "SELECT begin FROM t; SELECT 2;",
            "SELECT t.begin, x FROM t; SELECT 2;",
            "UPDATE t SET begin = 1 WHERE begin > 0; SELECT 2;",
            "SELECT * FROM t ORDER BY begin DESC; SELECT 2;",
            "SELECT begin AS started FROM t; SELECT 2;",
        ] {
            assert!(!contains_compound_block(identifier), "{identifier}");
        }
        assert!(!contains_compound_block("-- BEGIN x\nSELECT 1; SELECT 2;"));
    }

    #[test]
    fn keywords_in_names_literals_and_comments_do_not_reclassify_reads() {
        assert_safe_reads(&[
            "SELECT updated_at, deleted, insert_count, created_by FROM t",
            "SELECT * FROM t WHERE status = 'DELETE'",
            "SELECT * FROM t WHERE note = 'drop it; insert into x'",
            "SELECT \"update\", \"delete\" FROM \"insert\"",
            "SELECT `update`, `into` FROM `delete`",
            "SELECT [update] FROM [dbo].[delete]",
            "SELECT t.update, t.into FROM t",
            "SELECT 1 -- DELETE FROM t",
            "SELECT /* DROP TABLE t */ 1",
            "SELECT $$DELETE FROM t$$ AS body",
            "SELECT $body$DELETE$body$ AS a, $$DROP$$ AS b",
            "SELECT REPLACE(name, 'a', 'b'), TRUNCATE(price, 2) FROM t",
            "SELECT * FROM t FOR SYSTEM_TIME AS OF '2020-01-01'",
            "SELECT * FROM t FOR JSON PATH",
            r"SELECT 'it''s' AS a, E'don\'t' AS b FROM t",
            "WITH recent AS (SELECT * FROM t WHERE updated_at > now()) SELECT * FROM recent",
            "(SELECT 1) UNION (SELECT 2)",
        ]);
    }

    #[test]
    fn database_administration_queries_require_admin_governance() {
        for query in [
            "SYSTEM SHUTDOWN",
            "KILL QUERY WHERE query_id = 'abc'",
            "OPTIMIZE TABLE events FINAL",
            "ATTACH TABLE events",
            "UNDROP TABLE events",
            "MOVE USER alice TO another_access_storage",
            "BACKUP DATABASE analytics TO Disk('backups', 'analytics.zip')",
            "RESTORE DATABASE analytics FROM Disk('backups', 'analytics.zip')",
        ] {
            assert_eq!(
                classify_sql_execution(query),
                ExecutionClassification::Admin,
                "query must require admin governance: {query}"
            );
        }
    }

    #[test]
    fn disruptive_administration_queries_require_destructive_governance() {
        for query in [
            "DETACH TABLE events",
            "RENAME TABLE old TO new",
            "EXCHANGE TABLES old AND new",
        ] {
            assert_eq!(
                classify_sql_execution(query),
                ExecutionClassification::Destructive,
                "query must require destructive governance: {query}"
            );
        }
    }

    #[test]
    fn side_effecting_functions_are_not_reads() {
        let cases = [
            // PostgreSQL
            (
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT pg_cancel_backend(42)",
                ExecutionClassification::Admin,
            ),
            ("SELECT pg_reload_conf()", ExecutionClassification::Admin),
            (
                "SELECT set_config('search_path', 'x', false)",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT pg_read_file('/etc/passwd')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT pg_read_binary_file('pg_hba.conf')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT * FROM pg_ls_dir('.')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT pg_stat_file('postgresql.conf')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT lo_import('/etc/passwd')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT lo_export(16409, '/tmp/out')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT pg_drop_replication_slot('s')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT nextval('users_id_seq')",
                ExecutionClassification::Write,
            ),
            (
                "SELECT setval('users_id_seq', 1)",
                ExecutionClassification::Write,
            ),
            (
                "SELECT dblink_exec('dbname=x', 'DELETE FROM t')",
                ExecutionClassification::Write,
            ),
            (
                "SELECT * FROM dblink('dbname=x', 'DELETE FROM t RETURNING 1') AS t(x int)",
                ExecutionClassification::Write,
            ),
            ("SELECT pg_advisory_lock(1)", ExecutionClassification::Write),
            (
                "SELECT pg_notify('channel', 'payload')",
                ExecutionClassification::Write,
            ),
            ("SELECT lo_unlink(16409)", ExecutionClassification::Write),
            (
                "SELECT query_to_xml('DELETE FROM t RETURNING *', true, false, '')",
                ExecutionClassification::Write,
            ),
            // Schema-qualified, mixed case and inside other expressions.
            (
                "SELECT pg_catalog.setval('s', 1)",
                ExecutionClassification::Write,
            ),
            (
                "select Pg_Catalog.PG_TERMINATE_BACKEND(1)",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT coalesce(nextval('s'), 0)",
                ExecutionClassification::Write,
            ),
            (
                "WITH x AS (SELECT setval('s', 1)) SELECT * FROM x",
                ExecutionClassification::Write,
            ),
            (
                "EXPLAIN ANALYZE SELECT nextval('s')",
                ExecutionClassification::Write,
            ),
            // A quoted name can call anything.
            (
                "SELECT \"pg_terminate_backend\"(1)",
                ExecutionClassification::Write,
            ),
            ("SELECT `get_lock`('a', 1)", ExecutionClassification::Write),
            // MySQL / MariaDB
            (
                "SELECT GET_LOCK('deploy', 10)",
                ExecutionClassification::Write,
            ),
            ("SELECT RELEASE_ALL_LOCKS()", ExecutionClassification::Write),
            (
                "SELECT LOAD_FILE('/etc/passwd')",
                ExecutionClassification::Admin,
            ),
            ("SELECT sys_exec('id')", ExecutionClassification::Admin),
            // SQLite / DuckDB
            (
                "SELECT load_extension('/tmp/evil.so')",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT writefile('/tmp/x', 'data')",
                ExecutionClassification::Admin,
            ),
            ("INSTALL httpfs", ExecutionClassification::Admin),
            ("LOAD httpfs", ExecutionClassification::Admin),
            // SQL Server
            ("EXEC xp_cmdshell 'whoami'", ExecutionClassification::Admin),
            (
                "EXECUTE master.dbo.xp_regwrite 'HKEY_LOCAL_MACHINE', 'k', 'v', 'REG_SZ', 'x'",
                ExecutionClassification::Admin,
            ),
            (
                "EXEC sp_configure 'xp_cmdshell', 1",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT * FROM OPENROWSET(BULK 'C:\\secret.txt', SINGLE_CLOB) AS x",
                ExecutionClassification::Admin,
            ),
            (
                "SELECT * FROM OPENQUERY(linked, 'DELETE FROM t')",
                ExecutionClassification::Write,
            ),
        ];

        for (query, expected) in cases {
            assert_eq!(
                classify_sql_execution(query),
                expected,
                "unexpected class for: {query}"
            );
            assert!(
                !is_safe_read_query(query),
                "must not pass as a read: {query}"
            );
        }
    }

    #[test]
    fn plain_functions_and_function_like_names_stay_reads() {
        for query in [
            "SELECT count(*), lower(name) FROM users",
            "SELECT pg_sleep(0.1)",
            "SELECT nextval FROM sequences_report",
            "SELECT s.setval FROM s",
            "WITH cte(a) AS (SELECT 1) SELECT * FROM cte",
            "SELECT 'pg_terminate_backend(1)'",
            "SELECT pg_size_pretty(pg_database_size(current_database()))",
            "SELECT * FROM pg_stat_activity",
        ] {
            assert_eq!(
                classify_sql_execution(query),
                ExecutionClassification::Read,
                "must stay a read: {query}"
            );
        }
    }

    #[test]
    fn explain_analyze_of_a_write_is_classified_by_the_explained_statement() {
        for query in [
            "EXPLAIN ANALYZE DELETE FROM t",
            "EXPLAIN (ANALYZE) UPDATE t SET a = 1",
            "EXPLAIN (FORMAT JSON) ANALYZE DELETE FROM t",
            "EXPLAIN ANALYZE FORMAT=JSON DELETE FROM t",
            "EXPLAIN ANALYZE COPY (SELECT 1) TO '/tmp/x'",
            "EXPLAIN QUERY PLAN SELECT 1; DELETE FROM t",
            "EXPLAIN EXPLAIN ANALYZE DELETE FROM t",
        ] {
            assert!(
                !matches!(
                    classify_sql_execution(query),
                    ExecutionClassification::Read | ExecutionClassification::Metadata
                ),
                "must not pass as a read: {query}"
            );
        }
    }
}
