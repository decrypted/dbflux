//! Rating InfluxQL and Flux as the server reads them.
//!
//! A word only counts where the language can run it, so this lexes by each
//! language's own rules. Those rules decide what hides, all measured:
//!
//! - InfluxQL regexes can hold a quote, so `FROM /'/ ; SELECT ... INTO x ; ... /'/`
//!   writes while a scan that only knows strings sees one long literal. Flux has
//!   the same hole with `"`.
//! - InfluxQL statements need a `;`, so only the first word of each one and the
//!   reserved word `INTO` decide its class.
//! - Flux reaches a function through more than a call: `f = to`, `m = influxdb`
//!   then `m.to(...)`, `influxdb["to"](...)`, an aliased import, and a call inside
//!   a string's `${...}` all run it.
//!
//! It is lexical, not a parser, and cannot see what a server-side task or a
//! package added later does, so a token without write access stays the real
//! boundary. What it cannot place rates up.

use dbflux_core::{DangerousQueryKind, ExecutionClassification};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dialect {
    InfluxQl,
    Flux,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Lexeme {
    /// An unquoted word: uppercased in InfluxQL, whose keywords ignore case, and
    /// as written in Flux, which does not.
    Word(String),
    /// A string or quoted name, with its text. Never code.
    Text(String),
    Number,
    Regex,
    /// Flux's `|>`.
    Pipe,
    Symbol(char),
}

struct Lexed {
    lexemes: Vec<Lexeme>,
    /// A string, regex or comment ran to the end of the text.
    unterminated: bool,
}

/// Measured on InfluxDB 1.8: these are the only places where `/` opens a regex
/// and the query still compiles. Elsewhere it divides or fails the whole query.
fn influxql_regex_can_follow(previous: Option<&Lexeme>) -> bool {
    match previous {
        Some(Lexeme::Word(word)) => matches!(word.as_str(), "SELECT" | "FROM" | "BY"),
        Some(Lexeme::Symbol(symbol)) => matches!(symbol, ',' | '.' | '(' | '~'),
        _ => false,
    }
}

/// Flux reads a regex wherever an operand is expected.
fn flux_regex_can_follow(previous: Option<&Lexeme>) -> bool {
    match previous {
        None | Some(Lexeme::Pipe) => true,
        Some(Lexeme::Word(word)) => matches!(
            word.as_str(),
            "and" | "or" | "not" | "exists" | "return" | "if" | "then" | "else"
        ),
        Some(Lexeme::Symbol(symbol)) => !matches!(symbol, ')' | ']' | '}'),
        Some(Lexeme::Text(_) | Lexeme::Number | Lexeme::Regex) => false,
    }
}

enum FluxStringEnd {
    Closed,
    Interpolation,
    NoEnd,
}

/// Reads a Flux string from `index` up to its closing quote or the next `${`,
/// whose content is code.
fn flux_string_part(chars: &[char], index: &mut usize) -> (String, FluxStringEnd) {
    let mut text = String::new();

    while let Some(character) = chars.get(*index).copied() {
        match character {
            '"' => {
                *index += 1;
                return (text, FluxStringEnd::Closed);
            }
            '$' if chars.get(*index + 1) == Some(&'{') => {
                *index += 2;
                return (text, FluxStringEnd::Interpolation);
            }
            '\\' => {
                *index += 2;
            }
            _ => {
                text.push(character);
                *index += 1;
            }
        }
    }

    (text, FluxStringEnd::NoEnd)
}

fn lex(query: &str, dialect: Dialect) -> Lexed {
    let chars: Vec<char> = query.chars().collect();
    let char_at = |index: usize| chars.get(index).copied();

    let mut lexemes: Vec<Lexeme> = Vec::new();
    let mut unterminated = false;
    let mut index = 0;
    // One entry per `${` still open, holding its count of open `{`.
    let mut interpolations: Vec<usize> = Vec::new();

    while let Some(current) = char_at(index) {
        let next = char_at(index + 1);

        if current.is_whitespace() {
            index += 1;
            continue;
        }

        let opens_line_comment = match dialect {
            Dialect::InfluxQl => current == '-' && next == Some('-'),
            Dialect::Flux => current == '/' && next == Some('/'),
        };

        if opens_line_comment {
            // The server also ends one at `\r` (measured on InfluxQL). Ending it
            // there in Flux too costs nothing, as what follows only reads as code.
            while char_at(index).is_some_and(|character| !matches!(character, '\n' | '\r')) {
                index += 1;
            }
            continue;
        }

        // `/*` opens a comment even where a regex could follow: as a regex `*`
        // does not compile, which fails the whole query (measured).
        if dialect == Dialect::InfluxQl && current == '/' && next == Some('*') {
            index += 2;
            loop {
                match (char_at(index), char_at(index + 1)) {
                    (None, _) => {
                        unterminated = true;
                        break;
                    }
                    (Some('*'), Some('/')) => {
                        index += 2;
                        break;
                    }
                    _ => index += 1,
                }
            }
            continue;
        }

        let opens_regex = current == '/'
            && match dialect {
                Dialect::InfluxQl => influxql_regex_can_follow(lexemes.last()),
                Dialect::Flux => flux_regex_can_follow(lexemes.last()),
            };

        if opens_regex {
            index += 1;
            loop {
                match char_at(index) {
                    None | Some('\n') => {
                        unterminated = true;
                        break;
                    }
                    Some('\\') => index += 2,
                    Some('/') => {
                        index += 1;
                        break;
                    }
                    Some(_) => index += 1,
                }
            }
            lexemes.push(Lexeme::Regex);
            continue;
        }

        if dialect == Dialect::Flux && current == '"' {
            index += 1;
            let (text, end) = flux_string_part(&chars, &mut index);
            match end {
                FluxStringEnd::Closed => {}
                FluxStringEnd::Interpolation => interpolations.push(0),
                FluxStringEnd::NoEnd => unterminated = true,
            }
            lexemes.push(Lexeme::Text(text));
            continue;
        }

        if dialect == Dialect::InfluxQl && matches!(current, '\'' | '"') {
            let mut text = String::new();
            let mut closed = false;
            index += 1;

            while let Some(character) = char_at(index) {
                index += 1;

                if character == current {
                    closed = true;
                    break;
                }
                if character == '\\' {
                    index += 1;
                    continue;
                }
                text.push(character);
            }

            unterminated |= !closed;
            lexemes.push(Lexeme::Text(text));
            continue;
        }

        if dialect == Dialect::Flux
            && current == '{'
            && let Some(depth) = interpolations.last_mut()
        {
            *depth += 1;
        }

        if dialect == Dialect::Flux && current == '}' {
            match interpolations.last_mut() {
                Some(0) => {
                    interpolations.pop();
                    index += 1;
                    let (text, end) = flux_string_part(&chars, &mut index);
                    match end {
                        FluxStringEnd::Closed => {}
                        FluxStringEnd::Interpolation => interpolations.push(0),
                        FluxStringEnd::NoEnd => unterminated = true,
                    }
                    lexemes.push(Lexeme::Text(text));
                    continue;
                }
                Some(depth) => *depth -= 1,
                None => {}
            }
        }

        if current == '|' && next == Some('>') {
            index += 2;
            lexemes.push(Lexeme::Pipe);
            continue;
        }

        // A number ends where its digits do, so `value*1INTO` still shows `INTO`.
        if current.is_ascii_digit() {
            while char_at(index)
                .is_some_and(|character| character.is_ascii_digit() || character == '.')
            {
                index += 1;
            }
            lexemes.push(Lexeme::Number);
            continue;
        }

        if current.is_alphanumeric() || current == '_' {
            let mut word = String::new();
            while let Some(character) = char_at(index)
                && (character.is_alphanumeric() || character == '_')
            {
                word.push(character);
                index += 1;
            }

            lexemes.push(Lexeme::Word(match dialect {
                Dialect::InfluxQl => word.to_uppercase(),
                Dialect::Flux => word,
            }));
            continue;
        }

        lexemes.push(Lexeme::Symbol(current));
        index += 1;
    }

    unterminated |= !interpolations.is_empty();

    Lexed {
        lexemes,
        unterminated,
    }
}

/// The dialect `query` is written in, from its text alone: a language service
/// gets no query mode. It only has to hold where it matters: no Flux program can
/// open with `SELECT`, `SHOW` or `EXPLAIN` unless it assigns to that name, which
/// [`classify_influxql_statement`] rates up, and no InfluxQL statement can read
/// as Flux here and still compile.
pub(crate) fn detect_dialect(query: &str) -> Dialect {
    let flux = lex(query, Dialect::Flux);

    let opens_as_flux = match flux.lexemes.as_slice() {
        [Lexeme::Word(word), Lexeme::Text(_), ..] => word == "import",
        [Lexeme::Word(word), Lexeme::Symbol('('), ..] => word == "from",
        [Lexeme::Word(_), Lexeme::Symbol('='), ..] => true,
        [Lexeme::Word(word), Lexeme::Word(_), ..] => {
            matches!(word.as_str(), "import" | "package" | "option")
        }
        _ => false,
    };

    // A `|>` counts only where neither language hides it: `-- |>` is a comment and
    // `'|>'` a string in InfluxQL, and Flux has neither.
    let pipes_in_both = flux.lexemes.contains(&Lexeme::Pipe)
        && lex(query, Dialect::InfluxQl)
            .lexemes
            .contains(&Lexeme::Pipe);

    if opens_as_flux || pipes_in_both {
        Dialect::Flux
    } else {
        Dialect::InfluxQl
    }
}

fn influxql_statements(lexemes: &[Lexeme]) -> impl Iterator<Item = &[Lexeme]> {
    lexemes
        .split(|lexeme| *lexeme == Lexeme::Symbol(';'))
        .filter(|statement| !statement.is_empty())
}

fn contains_word(lexemes: &[Lexeme], word: &str) -> bool {
    lexemes
        .iter()
        .any(|lexeme| matches!(lexeme, Lexeme::Word(found) if found == word))
}

/// `INTO` is reserved, so no name can use it unquoted (measured), and in any
/// statement it can only be the clause that writes.
fn classify_influxql_statement(statement: &[Lexeme]) -> ExecutionClassification {
    use ExecutionClassification::{Admin, Destructive, Metadata, Read, Write};

    let [Lexeme::Word(head), rest @ ..] = statement else {
        return Write;
    };

    if rest.first() == Some(&Lexeme::Symbol('=')) {
        return Write;
    }

    let class = match head.as_str() {
        "SHOW" | "EXPLAIN" => Metadata,
        "SELECT" => Read,
        "DELETE" | "DROP" | "ALTER" => Destructive,
        "CREATE" | "GRANT" | "REVOKE" | "SET" | "KILL" => Admin,
        _ => Write,
    };

    if contains_word(rest, "INTO") {
        class.max(Write)
    } else {
        class
    }
}

pub(crate) fn classify_influxql(query: &str) -> ExecutionClassification {
    let Lexed {
        lexemes,
        unterminated,
    } = lex(query, Dialect::InfluxQl);

    let class = influxql_statements(&lexemes)
        .map(classify_influxql_statement)
        .fold(
            ExecutionClassification::Metadata,
            ExecutionClassification::max,
        );

    if unterminated {
        class.max(ExecutionClassification::Write)
    } else {
        class
    }
}

fn dangerous_influxql_statement(statement: &[Lexeme]) -> Option<DangerousQueryKind> {
    let [Lexeme::Word(head), rest @ ..] = statement else {
        return None;
    };

    match head.as_str() {
        "DROP" => Some(DangerousQueryKind::Drop),
        "ALTER" => Some(DangerousQueryKind::Alter),
        "DELETE" if !contains_word(rest, "WHERE") => Some(DangerousQueryKind::DeleteNoWhere),
        _ => None,
    }
}

/// `DROP` is irreversible regardless of any predicate. `DELETE` follows the
/// WHERE-presence convention used everywhere else: one with no `WHERE` matches
/// every point in the target. Several statements with one of these among them
/// read as a script, as they do in SQL.
pub(crate) fn detect_dangerous_influxql(query: &str) -> Option<DangerousQueryKind> {
    let lexed = lex(query, Dialect::InfluxQl);
    let statements: Vec<&[Lexeme]> = influxql_statements(&lexed.lexemes).collect();

    let found = statements
        .iter()
        .find_map(|statement| dangerous_influxql_statement(statement))?;

    Some(if statements.len() > 1 {
        DangerousQueryKind::Script
    } else {
        found
    })
}

/// What using a member of a Flux package does.
#[derive(Clone, Copy)]
enum PackageEffect {
    /// Nothing leaves the query.
    Reads,
    /// These members write or reach outside; the others are pure.
    Only(&'static [&'static str]),
    /// Every member but these writes or reaches outside. For a package whose
    /// purpose that is, so a member added later rates up.
    AllBut(&'static [&'static str]),
}

impl PackageEffect {
    fn member_acts(self, member: &str) -> bool {
        match self {
            Self::Reads => false,
            Self::Only(members) => members.contains(&member),
            Self::AllBut(members) => !members.contains(&member),
        }
    }
}

/// Every package in Flux's standard library (`stdlib/` at influxdata/flux
/// 2f8a2dbd), read for what its members do. `get`-style members count: they send
/// a request of the query's making to any URL, and `sql.from` and
/// `clickhouse.query` run text of any kind on another server. A package not
/// listed here is unknown, and rates up.
fn package_effect(path: &str) -> Option<PackageEffect> {
    use PackageEffect::{AllBut, Only, Reads};

    Some(match path {
        "experimental" => Only(&["to"]),
        "influxdata/influxdb" => Only(&["to", "wideTo"]),
        "influxdata/influxdb/monitor" => Only(&["check", "notify", "write", "log"]),
        "experimental/prometheus" => Only(&["scrape"]),

        "http" => AllBut(&["basicAuth", "pathEscape"]),
        "http/requests" | "experimental/http/requests" => AllBut(&["peek", "defaultConfig"]),
        "slack" => AllBut(&["validateColorString"]),
        "pagerduty" => AllBut(&[
            "actionFromLevel",
            "actionFromSeverity",
            "dedupKey",
            "severityFromLevel",
        ]),
        "contrib/rhajek/bigpanda" => AllBut(&["statusFromLevel"]),
        "contrib/sranka/opsgenie" => AllBut(&["respondersToJSON"]),
        "contrib/sranka/sensu" => AllBut(&["toSensuName"]),
        "experimental/http"
        | "experimental/influxdb"
        | "experimental/mqtt"
        | "experimental/csv"
        | "kafka"
        | "sql"
        | "pushbullet"
        | "contrib/bonitoo-io/alerta"
        | "contrib/bonitoo-io/servicenow"
        | "contrib/bonitoo-io/tickscript"
        | "contrib/bonitoo-io/victorops"
        | "contrib/bonitoo-io/zenoss"
        | "contrib/chobbs/discord"
        | "contrib/qxip/clickhouse"
        | "contrib/qxip/logql"
        | "contrib/sranka/teams"
        | "contrib/sranka/telegram"
        | "contrib/sranka/webexteams" => AllBut(&[]),

        "array"
        | "bitwise"
        | "csv"
        | "date"
        | "date/boundaries"
        | "dict"
        | "generate"
        | "interpolate"
        | "join"
        | "json"
        | "math"
        | "planner"
        | "profiler"
        | "regexp"
        | "runtime"
        | "sampledata"
        | "socket"
        | "strings"
        | "system"
        | "testing"
        | "testing/expect"
        | "timezone"
        | "types"
        | "universe"
        | "experimental/aggregate"
        | "experimental/array"
        | "experimental/bigtable"
        | "experimental/bitwise"
        | "experimental/date/boundaries"
        | "experimental/dynamic"
        | "experimental/geo"
        | "experimental/iox"
        | "experimental/json"
        | "experimental/oee"
        | "experimental/polyline"
        | "experimental/query"
        | "experimental/record"
        | "experimental/table"
        | "experimental/usage"
        | "influxdata/influxdb/sample"
        | "influxdata/influxdb/schema"
        | "influxdata/influxdb/secrets"
        | "influxdata/influxdb/tasks"
        | "influxdata/influxdb/v1"
        | "contrib/RohanSreerama5/naiveBayesClassifier"
        | "contrib/anaisdg/anomalydetection"
        | "contrib/anaisdg/statsmodels"
        | "contrib/bonitoo-io/hex"
        | "contrib/jsternberg/influxdb"
        | "contrib/qxip/hash"
        | "contrib/tomhollingworth/events" => Reads,

        _ => return None,
    })
}

struct FluxImport<'a> {
    name: &'a str,
    effect: Option<PackageEffect>,
}

/// The imports opening a Flux program, and where its body starts. Flux has no
/// imports anywhere else.
fn flux_imports(lexemes: &[Lexeme]) -> (Vec<FluxImport<'_>>, usize) {
    let mut imports = Vec::new();
    let mut index = 0;

    if matches!(lexemes.first(), Some(Lexeme::Word(word)) if word == "package") {
        index = 2;
    }

    loop {
        let found = match (
            lexemes.get(index),
            lexemes.get(index + 1),
            lexemes.get(index + 2),
        ) {
            (Some(Lexeme::Word(word)), Some(Lexeme::Text(path)), _) if word == "import" => {
                let name = path.rsplit('/').next().unwrap_or(path);
                Some((name, path, 2))
            }
            (Some(Lexeme::Word(word)), Some(Lexeme::Word(alias)), Some(Lexeme::Text(path)))
                if word == "import" =>
            {
                Some((alias.as_str(), path, 3))
            }
            _ => None,
        };

        let Some((name, path, length)) = found else {
            return (imports, index);
        };

        imports.push(FluxImport {
            name,
            effect: package_effect(path),
        });
        index += length;
    }
}

/// What the words of a Flux program do, or `None` when nothing in it acts.
///
/// A column is what stays out: `r.to`, `{r with to: 1}`, `["to"]` and `"to("`
/// never call anything. Any other bare `to` is the function or a name for it.
fn flux_effect(lexemes: &[Lexeme]) -> Option<ExecutionClassification> {
    let (imports, body_start) = flux_imports(lexemes);
    let imported = |name: &str| imports.iter().find(|import| import.name == name);

    if imports.iter().any(|import| import.effect.is_none()) {
        return Some(ExecutionClassification::Write);
    }

    let mut effect = None;

    for (index, lexeme) in lexemes.iter().enumerate().skip(body_start) {
        let Lexeme::Word(word) = lexeme else {
            continue;
        };

        let next = lexemes.get(index + 1);
        let is_call = next == Some(&Lexeme::Symbol('('));

        if word == "delete" && is_call {
            return Some(ExecutionClassification::Destructive);
        }

        let is_member = index
            .checked_sub(1)
            .and_then(|previous| lexemes.get(previous))
            == Some(&Lexeme::Symbol('.'));

        let acts = if is_member {
            let owner = index.checked_sub(2).and_then(|owner| lexemes.get(owner));

            match owner {
                Some(Lexeme::Word(owner)) => imported(owner)
                    .map_or(word == "to" && is_call, |import| {
                        import.effect.is_some_and(|effect| effect.member_acts(word))
                    }),
                _ => word == "to" && is_call,
            }
        } else if next == Some(&Lexeme::Symbol(':')) {
            false
        } else if word == "to" {
            true
        } else {
            // A package used as a value (`m = influxdb`, `influxdb["to"]`) can reach
            // any of its members, and those run (measured).
            next != Some(&Lexeme::Symbol('.'))
                && imported(word)
                    .is_some_and(|import| !matches!(import.effect, Some(PackageEffect::Reads)))
        };

        if acts {
            effect = Some(ExecutionClassification::Write);
        }
    }

    effect
}

fn calls_from(lexemes: &[Lexeme]) -> bool {
    lexemes
        .windows(2)
        .any(|pair| matches!(pair, [Lexeme::Word(word), Lexeme::Symbol('(')] if word == "from"))
}

pub(crate) fn classify_flux(query: &str) -> ExecutionClassification {
    let Lexed {
        lexemes,
        unterminated,
    } = lex(query, Dialect::Flux);

    if lexemes.is_empty() && !unterminated {
        return ExecutionClassification::Metadata;
    }

    if let Some(effect) = flux_effect(&lexemes) {
        return effect;
    }

    if !unterminated && calls_from(&lexemes) {
        ExecutionClassification::Read
    } else {
        ExecutionClassification::Write
    }
}

/// Flux's `delete()` takes a bucket, predicate, start and stop, with no row
/// identity comparable to SQL's `WHERE id = ...`, so any call is a broad delete.
pub(crate) fn detect_dangerous_flux(query: &str) -> Option<DangerousQueryKind> {
    let lexemes = lex(query, Dialect::Flux).lexemes;

    let calls_delete = lexemes
        .windows(2)
        .any(|pair| matches!(pair, [Lexeme::Word(word), Lexeme::Symbol('(')] if word == "delete"));

    calls_delete.then_some(DangerousQueryKind::DeleteNoWhere)
}

#[cfg(test)]
mod tests;
