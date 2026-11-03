//! Rating InfluxQL and Flux as the server reads them.
//!
//! A word only counts where the language can run it, so this lexes by each
//! language's own rules, following `influxql/scanner.go` and Flux's `scanner.rl`.
//! Those rules decide what hides, all measured on a server:
//!
//! - InfluxQL and Flux regexes can hold a quote, so `FROM /'/ ; SELECT ... INTO x ;
//!   ... /'/` writes while a scan that only knows strings sees one long literal.
//! - InfluxQL statements need a `;`, so only the first word of each one and the
//!   reserved word `INTO` decide its class.
//! - Flux reaches a function through more than a call: `f = to`, `m = influxdb`
//!   then `m.to(...)`, `influxdb["to"](...)`, an aliased import, and a call inside
//!   a string's `${...}` all run it.
//! - `--` is a comment only in InfluxQL. In Flux, `--1` is a number, and code
//!   after it on that line runs. The service does not know which language a
//!   request runs in, so what that comment hides counts too.
//!
//! It is lexical, not a parser, and cannot see what a server-side task or a
//! package added later does, so a token without write access stays the real
//! boundary. What it cannot place rates up. Reading a `/` or a quote wrongly here
//! only matters when the server accepts the text: it parses the whole query first,
//! so a text it refuses runs nothing.

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
    /// What each InfluxQL `--` comment took, which Flux reads as code.
    dash_comments: Vec<String>,
}

/// The positions InfluxQL's parser asks for a regex: a field, a source (also after
/// `WITH MEASUREMENT =` and a qualifying `.`), a `GROUP BY` dimension, a call
/// argument, and the right of `=~` and `!~`. A `/` elsewhere divides or fails the
/// whole query.
fn influxql_regex_can_follow(previous: Option<&Lexeme>) -> bool {
    match previous {
        Some(Lexeme::Word(word)) => matches!(word.as_str(), "SELECT" | "FROM" | "BY"),
        Some(Lexeme::Symbol(symbol)) => matches!(symbol, ',' | '.' | '(' | '~' | '='),
        _ => false,
    }
}

/// Flux reads a regex wherever an operand is expected. At the start of a statement
/// it does not (measured after `import`, `package` and a closed function).
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

struct Lexer {
    chars: Vec<char>,
    dialect: Dialect,
    index: usize,
    lexemes: Vec<Lexeme>,
    unterminated: bool,
    dash_comments: Vec<String>,
    /// One entry per `${` still open, holding its count of open `{`.
    interpolations: Vec<usize>,
}

impl Lexer {
    fn at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    fn current(&self) -> Option<char> {
        self.at(self.index)
    }

    fn following(&self) -> Option<char> {
        self.at(self.index + 1)
    }

    /// InfluxQL reads `\r` as a line end and Flux does not, and treating it as one
    /// in Flux would turn a comment's tail into code (measured: a `"` there hid the
    /// `to()` on the next line).
    fn skip_line_comment(&mut self) {
        let ends_line = |character: char| match self.dialect {
            Dialect::InfluxQl => matches!(character, '\n' | '\r'),
            Dialect::Flux => character == '\n',
        };

        let mut index = self.index;
        while self
            .at(index)
            .is_some_and(|character| !ends_line(character))
        {
            index += 1;
        }

        if self.dialect == Dialect::InfluxQl {
            let taken = self.chars.get(self.index + 2..index).unwrap_or_default();
            self.dash_comments.push(taken.iter().collect());
        }

        self.index = index;
    }

    fn skip_block_comment(&mut self) {
        self.index += 2;
        loop {
            match (self.current(), self.following()) {
                (None, _) => {
                    self.unterminated = true;
                    return;
                }
                (Some('*'), Some('/')) => {
                    self.index += 2;
                    return;
                }
                _ => self.index += 1,
            }
        }
    }

    /// Only `\/` is an escape in a regex in either language: after any other `\`
    /// the next character is read as usual, so `\\/` closes nothing. A line end
    /// fails the query in InfluxQL and is part of the regex in Flux.
    fn read_regex(&mut self) {
        self.index += 1;
        loop {
            match self.current() {
                None => {
                    self.unterminated = true;
                    break;
                }
                Some('\n' | '\r') if self.dialect == Dialect::InfluxQl => {
                    self.unterminated = true;
                    break;
                }
                Some('\\') if self.following() == Some('/') => self.index += 2,
                Some('/') => {
                    self.index += 1;
                    break;
                }
                Some(_) => self.index += 1,
            }
        }
        self.lexemes.push(Lexeme::Regex);
    }

    /// Reads a Flux string from the cursor up to its closing quote or the next
    /// `${`, whose content is code. A `$` takes the character after it along, so
    /// `$${` holds no interpolation (measured).
    fn flux_string_part(&mut self) -> (String, FluxStringEnd) {
        let mut text = String::new();

        while let Some(character) = self.current() {
            match (character, self.following()) {
                ('"', _) => {
                    self.index += 1;
                    return (text, FluxStringEnd::Closed);
                }
                ('$', Some('{')) => {
                    self.index += 2;
                    return (text, FluxStringEnd::Interpolation);
                }
                ('$', Some('"')) => self.index += 1,
                ('$' | '\\', _) => self.index += 2,
                _ => {
                    text.push(character);
                    self.index += 1;
                }
            }
        }

        (text, FluxStringEnd::NoEnd)
    }

    /// The `{` makes what follows an operand position, as the start of an
    /// interpolation is.
    fn read_flux_string_part(&mut self) {
        let (text, end) = self.flux_string_part();
        self.lexemes.push(Lexeme::Text(text));

        match end {
            FluxStringEnd::Closed => {}
            FluxStringEnd::Interpolation => {
                self.interpolations.push(0);
                self.lexemes.push(Lexeme::Symbol('{'));
            }
            FluxStringEnd::NoEnd => self.unterminated = true,
        }
    }

    fn read_influxql_quoted(&mut self, close: char) {
        let mut text = String::new();
        let mut closed = false;
        self.index += 1;

        while let Some(character) = self.current() {
            self.index += 1;

            if character == close {
                closed = true;
                break;
            }
            if character == '\\' {
                self.index += 1;
                continue;
            }
            text.push(character);
        }

        self.unterminated |= !closed;
        self.lexemes.push(Lexeme::Text(text));
    }

    /// Whether a `}` here closed an interpolation and went back into its string.
    fn close_flux_interpolation(&mut self) -> bool {
        match self.interpolations.last_mut() {
            Some(0) => {
                self.interpolations.pop();
                self.index += 1;
                self.lexemes.push(Lexeme::Symbol('}'));
                self.read_flux_string_part();
                true
            }
            Some(depth) => {
                *depth -= 1;
                false
            }
            None => false,
        }
    }

    /// A number ends where its digits do, so `value*1INTO` still shows `INTO`.
    fn read_number(&mut self) {
        while self
            .current()
            .is_some_and(|character| character.is_ascii_digit() || character == '.')
        {
            self.index += 1;
        }
        self.lexemes.push(Lexeme::Number);
    }

    fn read_word(&mut self) {
        let mut word = String::new();
        while let Some(character) = self.current()
            && (character.is_alphanumeric() || character == '_')
        {
            word.push(character);
            self.index += 1;
        }

        self.lexemes.push(Lexeme::Word(match self.dialect {
            Dialect::InfluxQl => word.to_uppercase(),
            Dialect::Flux => word,
        }));
    }

    fn opens_regex(&self) -> bool {
        match self.dialect {
            Dialect::InfluxQl => influxql_regex_can_follow(self.lexemes.last()),
            Dialect::Flux => flux_regex_can_follow(self.lexemes.last()),
        }
    }

    fn step(&mut self, current: char) {
        let next = self.following();
        let is_flux = self.dialect == Dialect::Flux;

        let opens_line_comment = if is_flux {
            current == '/' && next == Some('/')
        } else {
            current == '-' && next == Some('-')
        };

        if opens_line_comment {
            self.skip_line_comment();
            return;
        }

        // `/*` opens a comment even where a regex could follow: as a regex `*`
        // does not compile, which fails the whole query (measured).
        if !is_flux && current == '/' && next == Some('*') {
            self.skip_block_comment();
            return;
        }

        if current == '/' && self.opens_regex() {
            self.read_regex();
            return;
        }

        if is_flux && current == '"' {
            self.index += 1;
            self.read_flux_string_part();
            return;
        }

        if !is_flux && matches!(current, '\'' | '"') {
            self.read_influxql_quoted(current);
            return;
        }

        if is_flux
            && current == '{'
            && let Some(depth) = self.interpolations.last_mut()
        {
            *depth += 1;
        }

        if is_flux && current == '}' && self.close_flux_interpolation() {
            return;
        }

        if current == '|' && next == Some('>') {
            self.index += 2;
            self.lexemes.push(Lexeme::Pipe);
        } else if current.is_ascii_digit() {
            self.read_number();
        } else if current.is_alphanumeric() || current == '_' {
            self.read_word();
        } else {
            self.lexemes.push(Lexeme::Symbol(current));
            self.index += 1;
        }
    }
}

fn lex(query: &str, dialect: Dialect) -> Lexed {
    let mut lexer = Lexer {
        chars: query.chars().collect(),
        dialect,
        index: 0,
        lexemes: Vec::new(),
        unterminated: false,
        dash_comments: Vec::new(),
        interpolations: Vec::new(),
    };

    while let Some(current) = lexer.current() {
        if current.is_whitespace() {
            lexer.index += 1;
        } else {
            lexer.step(current);
        }
    }

    Lexed {
        unterminated: lexer.unterminated || !lexer.interpolations.is_empty(),
        lexemes: lexer.lexemes,
        dash_comments: lexer.dash_comments,
    }
}

/// The dialect `query` is written in, from its text alone: a language service
/// gets no query mode. It only has to hold where it matters: no Flux program can
/// open with `SELECT`, `SHOW` or `EXPLAIN` unless it assigns to that name, which
/// [`classify_influxql_statement`] rates up, or hides behind a `--`, which
/// [`rating_if_flux_behind_dash_comment`] does. No InfluxQL statement can read as
/// Flux here and still compile.
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

    // A `|>` counts only where neither language hides it: `'|>'` is a string in
    // InfluxQL, and Flux has none.
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

/// Whether `to` stands where a value does: `to(...)`, `{f: to}`, `x = to`. Between
/// words it is prose, as in `-- compare to yesterday`.
fn uses_to_as_a_value(lexemes: &[Lexeme]) -> bool {
    let is_word = |lexeme: Option<&Lexeme>| matches!(lexeme, Some(Lexeme::Word(_)));

    lexemes.iter().enumerate().any(|(index, lexeme)| {
        let previous = index.checked_sub(1).and_then(|index| lexemes.get(index));

        matches!(lexeme, Lexeme::Word(word) if word == "to")
            && !is_word(previous)
            && !is_word(lexemes.get(index + 1))
    })
}

/// What the `--` comments of text that reads as InfluxQL say about it running as
/// Flux, where they are code (measured: `--1 from(...) |> to(...)` writes).
#[derive(Clone, Copy, PartialEq, Eq)]
enum DashComments {
    /// Plain words, or nothing a Flux program would need. The rest of the text
    /// reads as InfluxQL, and no Flux program can open with that.
    Comments,
    /// A program could start here. A string or regex left open counts: what the
    /// InfluxQL reading takes for code can be inside it, and what it takes for a
    /// quoted name can be code.
    MaybeFlux,
    /// A `|>` or an import is Flux, or nothing at all.
    Flux,
}

fn read_dash_comments(query: &str) -> DashComments {
    let readings: Vec<Lexed> = lex(query, Dialect::InfluxQl)
        .dash_comments
        .iter()
        .map(|comment| lex(comment, Dialect::Flux))
        .collect();

    let is_flux = readings.iter().any(|reading| {
        reading.lexemes.contains(&Lexeme::Pipe) || !flux_imports(&reading.lexemes).is_empty()
    });

    if is_flux {
        return DashComments::Flux;
    }

    let may_be_flux = readings.iter().any(|reading| {
        reading.unterminated
            || reading.lexemes.contains(&Lexeme::Symbol('='))
            || uses_to_as_a_value(&reading.lexemes)
            || passes_a_remote_argument(&reading.lexemes)
    });

    if may_be_flux {
        DashComments::MaybeFlux
    } else {
        DashComments::Comments
    }
}

/// The rating of text that reads as InfluxQL with a `--` comment, in case it runs
/// as Flux. Which language runs it cannot be told from here. The comments decide
/// whether to ask, since an ordinary `WHERE host = 'a'` also lexes as Flux. The
/// whole text decides the answer, since a program behind `--1` goes on from there.
fn rating_if_flux_behind_dash_comment(query: &str) -> Option<ExecutionClassification> {
    let comments = read_dash_comments(query);

    if comments == DashComments::Comments {
        return None;
    }

    let effect = flux_effect(&lex(query, Dialect::Flux).lexemes);

    if comments == DashComments::Flux {
        Some(effect.unwrap_or(ExecutionClassification::Write))
    } else {
        effect
    }
}

/// The class of `query`, in the language its text reads as.
pub(crate) fn classify_execution(query: &str) -> ExecutionClassification {
    match detect_dialect(query) {
        Dialect::Flux => classify_flux(query),
        Dialect::InfluxQl => {
            let class = classify_influxql(query);

            rating_if_flux_behind_dash_comment(query).map_or(class, |flux| class.max(flux))
        }
    }
}

/// The confirmation `query` needs, in the language its text reads as.
pub(crate) fn detect_dangerous(query: &str) -> Option<DangerousQueryKind> {
    match detect_dialect(query) {
        Dialect::Flux => detect_dangerous_flux(query),
        Dialect::InfluxQl => {
            detect_dangerous_influxql(query).or_else(|| detect_dangerous_flux_behind_comment(query))
        }
    }
}

fn detect_dangerous_flux_behind_comment(query: &str) -> Option<DangerousQueryKind> {
    if read_dash_comments(query) == DashComments::Comments {
        None
    } else {
        detect_dangerous_flux(query)
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

fn classify_influxql(query: &str) -> ExecutionClassification {
    let Lexed {
        lexemes,
        unterminated,
        ..
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
fn detect_dangerous_influxql(query: &str) -> Option<DangerousQueryKind> {
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
/// 2f8a2dbd), read for what its members do. A member that reaches another host
/// counts, whatever it does there: `requests.get` and `usage.limits` send a request
/// of the query's making, `socket.from` opens a connection, and `sql.from` and
/// `clickhouse.query` run text of any kind on another server. A package not
/// listed here is unknown, and rates up.
fn package_effect(path: &str) -> Option<PackageEffect> {
    use PackageEffect::{AllBut, Only, Reads};

    Some(match path {
        "experimental" => Only(&["to"]),
        // Measured: 2.7 has no `universe.to`, but `to` is what the language calls
        // bare, so a version that adds the member must not read as pure.
        "universe" => Only(&["to"]),
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
        | "experimental/bigtable"
        | "experimental/usage"
        | "socket"
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
        | "strings"
        | "system"
        | "testing"
        | "testing/expect"
        | "timezone"
        | "types"
        | "experimental/aggregate"
        | "experimental/array"
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

/// The imports of a Flux program, wherever they stand, since an attribute or a
/// `package` clause can come first.
fn flux_imports(lexemes: &[Lexeme]) -> Vec<FluxImport<'_>> {
    lexemes
        .iter()
        .enumerate()
        .filter(|(_, lexeme)| matches!(lexeme, Lexeme::Word(word) if word == "import"))
        .filter_map(
            |(index, _)| match (lexemes.get(index + 1), lexemes.get(index + 2)) {
                (Some(Lexeme::Text(path)), _) => Some((path.rsplit('/').next()?, path)),
                (Some(Lexeme::Word(alias)), Some(Lexeme::Text(path))) => {
                    Some((alias.as_str(), path))
                }
                _ => None,
            },
        )
        .map(|(name, path)| FluxImport {
            name,
            effect: package_effect(path),
        })
        .collect()
}

/// Whether a call names `host` or `url` as an argument: `from(host: ...)`,
/// `influxdb.from(host: ...)` and `usage.limits(host: ...)` send a request there
/// (measured), whatever package holds the function. So do the shorthand
/// `from(bucket, host, token)` and a quoted key `from("host": ...)`, and any
/// quoted key rates up, as this cannot read the escapes one may hold. A record key
/// is one bracket further in, so `{r with host: "x"}` does not count.
fn passes_a_remote_argument(lexemes: &[Lexeme]) -> bool {
    let mut open: Vec<char> = Vec::new();

    for (index, lexeme) in lexemes.iter().enumerate() {
        match lexeme {
            Lexeme::Symbol(symbol @ ('(' | '[' | '{')) => open.push(*symbol),
            Lexeme::Symbol(')' | ']' | '}') => {
                open.pop();
            }
            Lexeme::Text(_)
                if open.last() == Some(&'(')
                    && lexemes.get(index + 1) == Some(&Lexeme::Symbol(':')) =>
            {
                return true;
            }
            Lexeme::Word(word)
                if matches!(word.as_str(), "host" | "url")
                    && open.last() == Some(&'(')
                    && index
                        .checked_sub(1)
                        .and_then(|previous| lexemes.get(previous))
                        != Some(&Lexeme::Symbol('.'))
                    && matches!(
                        lexemes.get(index + 1),
                        Some(Lexeme::Symbol(':' | ',' | ')'))
                    ) =>
            {
                return true;
            }
            _ => {}
        }
    }

    false
}

/// What the words of a Flux program do, or `None` when nothing in it acts.
///
/// A column is what stays out: `r.to`, `{r with to: 1}`, `["to"]` and `"to("`
/// never call anything. Any other bare `to` is the function or a name for it.
fn flux_effect(lexemes: &[Lexeme]) -> Option<ExecutionClassification> {
    let imports = flux_imports(lexemes);
    let imported = |name: &str| imports.iter().find(|import| import.name == name);

    // An attribute or a `builtin` is nothing a query needs, and this scan does not
    // follow what either one does.
    let cannot_follow = lexemes.contains(&Lexeme::Symbol('@'))
        || lexemes
            .iter()
            .any(|lexeme| matches!(lexeme, Lexeme::Word(word) if word == "builtin"));

    if cannot_follow
        || imports.iter().any(|import| import.effect.is_none())
        || passes_a_remote_argument(lexemes)
    {
        return Some(ExecutionClassification::Write);
    }

    let mut effect = None;

    for (index, lexeme) in lexemes.iter().enumerate() {
        let Lexeme::Word(word) = lexeme else {
            continue;
        };

        let next = lexemes.get(index + 1);
        let is_call = next == Some(&Lexeme::Symbol('('));

        if word == "delete" && is_call {
            return Some(ExecutionClassification::Destructive);
        }

        let previous = index
            .checked_sub(1)
            .and_then(|previous| lexemes.get(previous));

        if matches!(previous, Some(Lexeme::Word(word)) if word == "import") {
            continue;
        }

        let is_member = previous == Some(&Lexeme::Symbol('.'));

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

fn calls_word(lexemes: &[Lexeme], called: &str) -> bool {
    lexemes
        .windows(2)
        .any(|pair| matches!(pair, [Lexeme::Word(word), Lexeme::Symbol('(')] if word == called))
}

fn classify_flux(query: &str) -> ExecutionClassification {
    let Lexed {
        lexemes,
        unterminated,
        ..
    } = lex(query, Dialect::Flux);

    if lexemes.is_empty() && !unterminated {
        return ExecutionClassification::Metadata;
    }

    if let Some(effect) = flux_effect(&lexemes) {
        return effect;
    }

    if !unterminated && calls_word(&lexemes, "from") {
        ExecutionClassification::Read
    } else {
        ExecutionClassification::Write
    }
}

/// Flux's `delete()` takes a bucket, predicate, start and stop, with no row
/// identity comparable to SQL's `WHERE id = ...`, so any call is a broad delete.
fn detect_dangerous_flux(query: &str) -> Option<DangerousQueryKind> {
    let lexemes = lex(query, Dialect::Flux).lexemes;

    calls_word(&lexemes, "delete").then_some(DangerousQueryKind::DeleteNoWhere)
}

#[cfg(test)]
mod tests;
