//! Rating a batch as SQL Server runs it.
//!
//! The shared classifier reads what other dialects share, and for those a
//! statement starts after a `;`. T-SQL differs in ways that decide what runs:
//!
//! - No `;` is needed, so a statement can start at any word.
//! - Its tokenizer needs no space around a number or a quoted token, so
//!   `SELECT 1DELETE FROM t` and `SELECT 'a'EXEC('...')` are two statements.
//! - An unknown first word is a call to a procedure of that name, and when the
//!   call fails the batch goes on, so `DESCRIBE x DELETE FROM t` deletes.
//!
//! So this lexes by T-SQL's own rules and rates by every word. What it rates up:
//! the reserved words outside the read grammar, wherever they appear, the
//! statement forms of a few unreserved words, and text with no end. Any other
//! word passes, as it is a name or an alias in a read. It is lexical, not a
//! parser, and cannot see what a called procedure or function does, so a
//! database role without write privileges stays the real boundary.

use dbflux_policy::ExecutionClassification;

use super::language_service::DangerousQueryKind;
use super::safety::{classify_procedure_name, side_effecting_function};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Lexeme {
    /// An unquoted word, uppercased.
    Word(String),
    /// A literal, a quoted name, or a `@variable` or `#temp` name: never code.
    Opaque,
    Symbol(char),
}

struct Lexed {
    lexemes: Vec<Lexeme>,
    /// A string, quoted name or comment ran to the end of the text.
    unterminated: bool,
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '@' | '#' | '$')
}

/// The server ends a line comment at `\r` as well as `\n`. Ending it early for
/// the others costs nothing: what follows is only read as code here.
fn ends_line_comment(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

fn push_word(chars: &[char], lexemes: &mut Vec<Lexeme>) {
    let word: String = chars.iter().collect();
    lexemes.push(Lexeme::Word(word.to_uppercase()));
}

fn lex(sql: &str) -> Lexed {
    let chars: Vec<char> = sql.chars().collect();
    let char_at = |index: usize| chars.get(index).copied();
    let is_digit_at = |index: usize| char_at(index).is_some_and(|c| c.is_ascii_digit());
    let word_end = |start: usize| {
        let mut end = start;
        while char_at(end).is_some_and(is_word_char) {
            end += 1;
        }
        end
    };

    let mut lexemes = Vec::new();
    let mut unterminated = false;
    let mut index = 0;

    while let Some(current) = char_at(index) {
        if current.is_whitespace() {
            index += 1;
            continue;
        }

        if current == '-' && char_at(index + 1) == Some('-') {
            while char_at(index).is_some_and(|character| !ends_line_comment(character)) {
                index += 1;
            }
            continue;
        }

        if current == '/' && char_at(index + 1) == Some('*') {
            let mut depth = 1usize;
            index += 2;

            while depth > 0 {
                match (char_at(index), char_at(index + 1)) {
                    (None, _) => {
                        unterminated = true;
                        break;
                    }
                    (Some('/'), Some('*')) => {
                        depth += 1;
                        index += 2;
                    }
                    (Some('*'), Some('/')) => {
                        depth -= 1;
                        index += 2;
                    }
                    _ => index += 1,
                }
            }
            continue;
        }

        let close = match current {
            '\'' | '"' => Some(current),
            '[' => Some(']'),
            _ => None,
        };

        if let Some(close) = close {
            index += 1;
            let mut closed = false;

            while let Some(character) = char_at(index) {
                index += 1;

                if character == close {
                    if char_at(index) == Some(close) {
                        index += 1;
                        continue;
                    }
                    closed = true;
                    break;
                }
            }

            unterminated |= !closed;
            lexemes.push(Lexeme::Opaque);
            continue;
        }

        // `$` opens a money literal or a pseudocolumn (`$action`). What follows
        // a pseudocolumn's `$` still counts as a word.
        if current == '$' && !is_digit_at(index + 1) {
            let is_money_fraction = char_at(index + 1) == Some('.') && is_digit_at(index + 2);

            if !is_money_fraction {
                let end = word_end(index + 1);
                if end > index + 1 {
                    push_word(chars.get(index + 1..end).unwrap_or_default(), &mut lexemes);
                } else {
                    lexemes.push(Lexeme::Opaque);
                }
                index = end;
                continue;
            }
        }

        let starts_number = current.is_ascii_digit()
            || current == '$'
            || (current == '.' && is_digit_at(index + 1));

        if starts_number {
            if current == '$' {
                index += 1;
            }

            if char_at(index) == Some('0') && matches!(char_at(index + 1), Some('x' | 'X')) {
                index += 2;
                while char_at(index).is_some_and(|c| c.is_ascii_hexdigit()) {
                    index += 1;
                }
                lexemes.push(Lexeme::Opaque);
                continue;
            }

            while is_digit_at(index) {
                index += 1;
            }
            if char_at(index) == Some('.') {
                index += 1;
                while is_digit_at(index) {
                    index += 1;
                }
            }

            if matches!(char_at(index), Some('e' | 'E')) {
                let digits_at = match char_at(index + 1) {
                    Some('+' | '-') => index + 2,
                    _ => index + 1,
                };

                if is_digit_at(digits_at) {
                    index = digits_at;
                    while is_digit_at(index) {
                        index += 1;
                    }
                } else {
                    // `1E` alone is a number, so `1EEXEC` opens `EXEC`, while
                    // `1EXEC` is measured as `1E` and an alias. Both readings
                    // count, since either one is only ever a reason to rate up.
                    let end = word_end(index);
                    push_word(chars.get(index..end).unwrap_or_default(), &mut lexemes);
                    index += 1;
                }
            }

            lexemes.push(Lexeme::Opaque);
            continue;
        }

        if matches!(current, '@' | '#') {
            index = word_end(index);
            lexemes.push(Lexeme::Opaque);
            continue;
        }

        if is_word_char(current) {
            let end = word_end(index);
            push_word(chars.get(index..end).unwrap_or_default(), &mut lexemes);
            index = end;
            continue;
        }

        lexemes.push(Lexeme::Symbol(current));
        index += 1;
    }

    Lexed {
        lexemes,
        unterminated,
    }
}

/// Words SQL Server has no statement for, which the shared classifier rates as
/// reads because other dialects do.
fn is_implicit_call_word(word: &str) -> bool {
    matches!(word, "EXPLAIN" | "DESC" | "DESCRIBE" | "SHOW")
}

/// Whether `lexemes` opens a statement with such a word, after any symbols.
fn opens_with_implicit_call(lexemes: &[Lexeme]) -> bool {
    lexemes
        .iter()
        .find(|lexeme| !matches!(lexeme, Lexeme::Symbol(_)))
        .is_some_and(|lexeme| matches!(lexeme, Lexeme::Word(word) if is_implicit_call_word(word)))
}

const UNRESERVED_STATEMENT_WORDS: &[&str] = &["SEND", "RECEIVE", "MOVE", "GET"];

/// The unreserved word that opens a Service Broker statement in `lexemes`, if
/// one does. The server requires a `;` before these: `SELECT 1 RECEIVE * FROM q`
/// does not compile, since `RECEIVE` reads as an alias. So only this position
/// counts, and a table or column of that name stays a read.
fn unreserved_statement_word(lexemes: &[Lexeme]) -> Option<&'static str> {
    let Some(Lexeme::Word(opening)) = lexemes
        .iter()
        .find(|lexeme| !matches!(lexeme, Lexeme::Symbol(_)))
    else {
        return None;
    };

    UNRESERVED_STATEMENT_WORDS
        .iter()
        .find(|word| **word == opening.as_str())
        .copied()
}

/// Whether the `CONVERSATION` at `index` is the statement `END CONVERSATION`,
/// which needs no `;` before it, as `END` is reserved. It is an alias when what
/// follows shows that (`CASE ... END conversation FROM t`); anything else counts.
fn is_end_conversation(lexemes: &[Lexeme], index: usize) -> bool {
    let after_end = index
        .checked_sub(1)
        .and_then(|previous| lexemes.get(previous))
        .is_some_and(|previous| matches!(previous, Lexeme::Word(word) if word == "END"));

    let is_alias = match lexemes.get(index + 1) {
        None => true,
        Some(Lexeme::Opaque) => false,
        Some(Lexeme::Symbol(symbol)) => *symbol != '(',
        Some(Lexeme::Word(word)) => matches!(
            word.as_str(),
            "FROM"
                | "WHERE"
                | "GROUP"
                | "ORDER"
                | "HAVING"
                | "UNION"
                | "EXCEPT"
                | "INTERSECT"
                | "OPTION"
                | "FOR"
                | "JOIN"
                | "INNER"
                | "LEFT"
                | "RIGHT"
                | "FULL"
                | "CROSS"
                | "OUTER"
                | "ON"
                | "AND"
                | "OR"
                | "AS"
                | "WHEN"
                | "THEN"
                | "ELSE"
                | "END"
                | "ASC"
                | "DESC"
        ),
    };

    after_end && !is_alias
}

/// `USE` only counts as a query hint where no statement can start: right after
/// the `(` or `,` of an option list, and followed by `HINT` or `PLAN`. Measured:
/// `SELECT 1 USE HINT` looks for a database named `HINT`.
fn is_use_query_hint(lexemes: &[Lexeme], index: usize) -> bool {
    let after_list_symbol = index
        .checked_sub(1)
        .and_then(|previous| lexemes.get(previous))
        .is_some_and(|previous| matches!(previous, Lexeme::Symbol('(' | ',')));
    let before_hint = lexemes
        .get(index + 1)
        .is_some_and(|next| matches!(next, Lexeme::Word(word) if word == "HINT" || word == "PLAN"));

    after_list_symbol && before_hint
}

const DESTRUCTIVE_WORDS: &[&str] = &["DROP", "TRUNCATE", "ALTER"];

const ADMIN_WORDS: &[&str] = &[
    "CREATE",
    "GRANT",
    "REVOKE",
    "DENY",
    "BACKUP",
    "RESTORE",
    "DUMP",
    "LOAD",
    "KILL",
    "SHUTDOWN",
    "DBCC",
    "RECONFIGURE",
    "BULK",
    "CHECKPOINT",
    "SETUSER",
    "REVERT",
    "OPENROWSET",
    "OPENDATASOURCE",
];

const WRITE_WORDS: &[&str] = &[
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "INTO",
    "OPENQUERY",
    "UPDATETEXT",
    "WRITETEXT",
    "READTEXT",
    "SET",
    "USE",
    "WAITFOR",
    "COMMIT",
    "ROLLBACK",
    "BEGIN",
    "SAVE",
    "TRAN",
    "TRANSACTION",
    "DISTRIBUTED",
    "DECLARE",
    "IF",
    "WHILE",
    "GOTO",
    "RETURN",
    "PRINT",
    "RAISERROR",
    "BREAK",
    "CONTINUE",
    "EXIT",
    "OPEN",
    "CLOSE",
    "DEALLOCATE",
    "CURSOR",
    "ADD",
    "AUTHORIZATION",
    "CASCADE",
    "CHECK",
    "CLUSTERED",
    "NONCLUSTERED",
    "COLUMN",
    "COMPUTE",
    "CONSTRAINT",
    "DATABASE",
    "DISK",
    "ERRLVL",
    "EXTERNAL",
    "FILE",
    "FILLFACTOR",
    "FOREIGN",
    "FUNCTION",
    "IDENTITY_INSERT",
    "LINENO",
    "NOCHECK",
    "OFF",
    "OFFSETS",
    "PRIMARY",
    "PROC",
    "PROCEDURE",
    "PUBLIC",
    "READ",
    "REFERENCES",
    "REPLICATION",
    "RESTRICT",
    "ROWCOUNT",
    "RULE",
    "SCHEMA",
    "SECURITYAUDIT",
    "STATISTICS",
    "TABLE",
    "TEXTSIZE",
    "TRIGGER",
    "TSEQUAL",
    "UNIQUE",
    "VIEW",
];

/// The class of `word` with its name, or `None` for a word a read may hold.
///
/// All of these are T-SQL reserved words, so no name can use one without quotes,
/// also after a `.` (measured: `SELECT t.exec FROM t` does not compile), and a
/// listed word is always the keyword. They are the reserved words outside the
/// read grammar. `DEFAULT` is left out as well, since it is a value in a call
/// (`fn(DEFAULT)`) and every write using it holds a listed word.
///
/// Unreserved words are not here, as they are legal names. See
/// [`unreserved_statement_word`] and [`is_end_conversation`].
fn classify_word(word: &str) -> Option<(ExecutionClassification, &'static str)> {
    use ExecutionClassification::{Admin, Destructive, Write};

    [
        (Destructive, DESTRUCTIVE_WORDS),
        (Admin, ADMIN_WORDS),
        (Write, WRITE_WORDS),
    ]
    .into_iter()
    .find_map(|(class, words)| {
        words
            .iter()
            .find(|listed| **listed == word)
            .map(|listed| (class, *listed))
    })
}

/// What a batch rates, and the first word found at that rating.
struct Rating {
    class: ExecutionClassification,
    word: Option<&'static str>,
}

impl Rating {
    fn raise(&mut self, class: ExecutionClassification, word: Option<&'static str>) {
        let raised = class.max(self.class) != self.class;

        if raised || (class == self.class && self.word.is_none()) {
            self.class = class;
            self.word = word;
        }
    }
}

fn rate(lexed: &Lexed) -> Rating {
    let lexemes = &lexed.lexemes;
    let mut rating = Rating {
        class: ExecutionClassification::Read,
        word: None,
    };

    if lexed.unterminated {
        rating.raise(ExecutionClassification::Write, None);
    }

    for statement in lexemes.split(|lexeme| *lexeme == Lexeme::Symbol(';')) {
        if opens_with_implicit_call(statement) {
            rating.raise(ExecutionClassification::Write, None);
        }

        if let Some(word) = unreserved_statement_word(statement) {
            rating.raise(ExecutionClassification::Write, Some(word));
        }
    }

    for (index, lexeme) in lexemes.iter().enumerate() {
        let Lexeme::Word(word) = lexeme else {
            continue;
        };
        let following = lexemes.get(index + 1..).unwrap_or_default();

        if following.first() == Some(&Lexeme::Symbol('('))
            && let Some(found) = side_effecting_function(word)
        {
            rating.raise(found, None);
        }

        match word.as_str() {
            "USE" if is_use_query_hint(lexemes, index) => {}
            "CONVERSATION" if is_end_conversation(lexemes, index) => {
                rating.raise(ExecutionClassification::Write, Some("END CONVERSATION"));
            }
            "EXEC" => rating.raise(
                classify_procedure_name(called_procedure(following)),
                Some("EXEC"),
            ),
            "EXECUTE" => rating.raise(
                classify_procedure_name(called_procedure(following)),
                Some("EXECUTE"),
            ),
            _ => {
                if let Some((class, name)) = classify_word(word) {
                    rating.raise(class, Some(name));
                }
            }
        }
    }

    rating
}

/// Rates `sql` by every word in it as SQL Server tokenizes it, for its language
/// service to raise the shared class with. `Read` when nothing raises it.
pub fn classify_tsql_execution(sql: &str) -> ExecutionClassification {
    rate(&lex(sql)).class
}

/// Whether a batch opens with what reads as a query.
fn opens_as_read(lexemes: &[Lexeme]) -> bool {
    lexemes
        .iter()
        .find(|lexeme| !matches!(lexeme, Lexeme::Symbol(_)))
        .is_some_and(
            |lexeme| matches!(lexeme, Lexeme::Word(word) if word == "SELECT" || word == "WITH"),
        )
}

/// The confirmation a batch needs when its first word says "read" and the rest
/// says otherwise: the shared danger check only looks at the first keyword of
/// each `;`-separated piece, and in T-SQL a statement can start at any word.
///
/// A batch the driver refuses for its implicit call needs no confirmation, as
/// it would not run after one. A batch that opens with a write is plainly one.
pub fn tsql_write_behind_read(sql: &str) -> Option<DangerousQueryKind> {
    let lexed = lex(sql);

    if !opens_as_read(&lexed.lexemes) || has_implicit_call(&lexed.lexemes) {
        return None;
    }

    let Rating { class, word } = rate(&lexed);

    (class != ExecutionClassification::Read).then_some(DangerousQueryKind::WriteBehindRead { word })
}

/// The confirmation a batch needs for a `DROP` or `TRUNCATE` behind whatever opens
/// it: `DELETE FROM t WHERE id = 1 DROP TABLE u` drops `u`, and the shared danger
/// check only reads the `DELETE`. Both words are reserved, so a listed one is the
/// keyword, and one in a string, comment or quoted name is not a word here.
///
/// `ALTER TABLE t DROP COLUMN a` is `ALTER`'s own grammar, so a batch opening with
/// `ALTER` keeps the prompt it has. A batch the driver refuses for its implicit
/// call does not ask, as it would not run after a yes.
pub fn tsql_drop_in_batch(sql: &str) -> Option<DangerousQueryKind> {
    let lexed = lex(sql);

    let opens_with_alter = lexed
        .lexemes
        .iter()
        .find(|lexeme| !matches!(lexeme, Lexeme::Symbol(_)))
        .is_some_and(|lexeme| matches!(lexeme, Lexeme::Word(word) if word == "ALTER"));

    if opens_with_alter || has_implicit_call(&lexed.lexemes) {
        return None;
    }

    lexed.lexemes.iter().find_map(|lexeme| match lexeme {
        Lexeme::Word(word) if word == "DROP" => Some(DangerousQueryKind::Drop),
        Lexeme::Word(word) if word == "TRUNCATE" => Some(DangerousQueryKind::Truncate),
        _ => None,
    })
}

/// The last word of the unquoted, possibly qualified name in `lexemes`.
fn called_procedure(lexemes: &[Lexeme]) -> Option<&str> {
    lexemes
        .iter()
        .take_while(|lexeme| matches!(lexeme, Lexeme::Word(_) | Lexeme::Symbol('.')))
        .filter_map(|lexeme| match lexeme {
            Lexeme::Word(word) => Some(word.as_str()),
            _ => None,
        })
        .last()
}

/// Whether a statement of `sql` opens with a word SQL Server would take for a
/// procedure call and then run the rest of the batch after. The driver refuses
/// these, since no such procedure is what anyone typing them meant.
pub fn tsql_batch_has_implicit_call(sql: &str) -> bool {
    has_implicit_call(&lex(sql).lexemes)
}

fn has_implicit_call(lexemes: &[Lexeme]) -> bool {
    lexemes
        .split(|lexeme| *lexeme == Lexeme::Symbol(';'))
        .any(opens_with_implicit_call)
}

#[cfg(test)]
mod tests;
