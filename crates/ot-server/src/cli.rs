//! The `opentally` command line: start the server, or look after a book directly.
//!
//! Every command prints plain text, or JSON with `--json`, and exits with a stable code
//! so scripts and AI agents can rely on it (see [`ExitCode`]). Commands other than
//! `serve` open the book themselves, so stop a running server first.

use std::{
    fs,
    io::{BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand};
use ot_core::auth::{Role, Scope};
use ot_store::{
    Store, StoreError,
    auth::{NewUserRecord, UserInfo, UserUpdate},
    migrate::{self, MigrationError},
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::auth::{self, AuthError};

#[derive(Debug, Parser)]
#[command(
    name = "opentally",
    version,
    about = "OpenTally accounting server and book tools",
    long_about = "OpenTally accounting server and book tools.\n\n\
        Without a command, `opentally` starts the server (the same as `opentally serve`). \
        Other commands open the book directly, so stop a running server first."
)]
pub struct Cli {
    /// The book file. Defaults to $OPENTALLY_DB, then $VOUCHER_DB, then data/vouchers.duckdb.
    #[arg(long, global = true, value_name = "FILE")]
    pub db: Option<PathBuf>,

    /// Print JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the web server (the default). Configured by OPENTALLY_* environment variables.
    Serve,
    /// Apply pending schema upgrades, keeping a backup copy first.
    Migrate,
    /// Show the book's company, schema version and number of users.
    Info,
    /// Manage users.
    #[command(subcommand)]
    User(UserCommand),
    /// Manage API tokens.
    #[command(subcommand)]
    Token(TokenCommand),
    /// Run a stdio MCP server for Claude Desktop, Claude Code and other MCP clients.
    /// It relays to a running OpenTally; set OPENTALLY_PORT, OPENTALLY_API_TOKEN and
    /// OPENTALLY_MCP_WRITES=1 as needed.
    Mcp {
        /// Relay to this OpenTally instead of http://127.0.0.1:$OPENTALLY_PORT.
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },
    /// Chart of accounts templates.
    #[command(subcommand)]
    Chart(ChartCommand),
    /// Number series that name documents, e.g. SINV-1001.
    #[command(subcommand)]
    Series(SeriesCommand),
    /// Customers and suppliers.
    #[command(subcommand)]
    Party(PartyCommand),
    /// Items you buy and sell.
    #[command(subcommand)]
    Item(ItemCommand),
    /// Units of measure.
    #[command(subcommand)]
    Unit(UnitCommand),
    /// Copy the book to a backup file.
    Backup {
        /// Where to write the copy.
        to: PathBuf,
        /// Replace the file if it exists.
        #[arg(long)]
        force: bool,
    },
    /// Replace the book with a backup. The current book is kept as <book>.before-restore.
    Restore {
        /// The backup file to restore.
        from: PathBuf,
        /// Confirm that the current book should be replaced.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ItemCommand {
    /// List items, optionally those matching text in the name, code, HSN or barcode.
    List {
        #[arg(long)]
        search: Option<String>,
    },
    /// Add an item. Its sales and purchase accounts come from the company defaults.
    Add {
        name: String,
        /// product or service (default product).
        #[arg(long = "type", default_value = "product")]
        item_type: String,
        /// The unit it is counted in (default Unit).
        #[arg(long)]
        unit: Option<String>,
        /// The rate in the company currency, e.g. 125.50.
        #[arg(long)]
        rate: Option<String>,
        /// The HSN/SAC code.
        #[arg(long)]
        hsn: Option<String>,
    },
    /// Delete an item by its ID, as `item list --json` shows it.
    Delete { id: i64 },
}

#[derive(Debug, Subcommand)]
pub enum UnitCommand {
    /// List the units of measure.
    List,
    /// Add a unit of measure.
    Add {
        name: String,
        /// Quantities in this unit must be whole numbers.
        #[arg(long)]
        whole: bool,
    },
}

fn item_text(item: &ot_store::items::Item) -> String {
    format!(
        "{:>5}  {:<32} {:>12} per {:<6} {}",
        item.id,
        item.name,
        item.rate,
        item.unit,
        item.hsn_code.as_deref().unwrap_or("")
    )
}

#[derive(Debug, Subcommand)]
pub enum PartyCommand {
    /// List parties, optionally only customers or suppliers, or those matching text.
    List {
        /// customer (includes both), supplier (includes both) or both.
        #[arg(long)]
        role: Option<String>,
        /// Text in the name, email, phone or GSTIN.
        #[arg(long)]
        search: Option<String>,
    },
    /// Add a party. Its default account and currency come from the company settings.
    Add {
        name: String,
        /// customer, supplier or both (default both).
        #[arg(long, default_value = "both")]
        role: String,
        /// A GSTIN makes the party a registered regular GST party.
        #[arg(long)]
        gstin: Option<String>,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        phone: Option<String>,
    },
    /// Delete a party by its ID, as `party list --json` shows it.
    Delete { id: i64 },
}

fn party_role(text: &str) -> Result<ot_store::parties::PartyRole, CliError> {
    serde_json::from_value(json!(text)).map_err(|_| {
        CliError::new(
            ExitCode::Invalid,
            format!("Unknown role {text}; use customer, supplier or both."),
        )
    })
}

fn party_text(party: &ot_store::parties::Party) -> String {
    format!(
        "{:>5}  {:<32} {:<9} {}",
        party.id,
        party.name,
        party.role.key(),
        party.gstin.as_deref().unwrap_or("")
    )
}

#[derive(Debug, Subcommand)]
pub enum SeriesCommand {
    /// List the number series and the next name each will give.
    List,
    /// Add a number series. The first series of a document type becomes its default.
    Add {
        /// The prefix, e.g. GST-26-. It cannot contain / = ? & %.
        prefix: String,
        /// The document type: journal_entry, payment, sales_invoice, purchase_invoice,
        /// sales_quote, purchase_receipt, shipment, stock_movement or pricing_rule.
        #[arg(long = "type", value_name = "TYPE")]
        reference_type: String,
        /// The first number (default 1001).
        #[arg(long)]
        start: Option<u64>,
        /// How many digits to pad the number to (default 4).
        #[arg(long)]
        pad: Option<u8>,
        /// Make it the default series for its document type.
        #[arg(long)]
        default: bool,
    },
    /// Change a series' start, padding, or make it the default for its type.
    Set {
        prefix: String,
        #[arg(long)]
        start: Option<u64>,
        #[arg(long)]
        pad: Option<u8>,
        #[arg(long)]
        default: bool,
    },
    /// Delete a series that has not named any document and is not a default.
    Delete { prefix: String },
}

fn series_text(row: &ot_store::series::SeriesRow) -> String {
    format!(
        "{:<18} {:<16} next {}{}",
        row.prefix,
        row.reference_label,
        row.next_name,
        if row.is_default { "  (default)" } else { "" }
    )
}

#[derive(Debug, Subcommand)]
pub enum ChartCommand {
    /// List the chart templates a book can start from.
    List,
    /// Add a template's groups and ledgers to the book. Existing ones are kept.
    Apply {
        /// The template's name, as `chart list` shows it.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// Add a user. The password is read from standard input.
    Add(NewUserArgs),
    /// List users.
    List,
    /// Change a user's name, roles, access or password.
    Set(SetUserArgs),
}

#[derive(Debug, Args)]
pub struct NewUserArgs {
    pub username: String,
    /// Full name, shown in the app and in voucher history.
    #[arg(long = "name")]
    pub full_name: String,
    /// A role; repeat for more. One of system_manager, books_manager, books_user,
    /// books_cashier, viewer.
    #[arg(long = "role", required = true, value_parser = parse_role)]
    pub roles: Vec<Role>,
}

#[derive(Debug, Args)]
pub struct SetUserArgs {
    pub username: String,
    #[arg(long = "name")]
    pub full_name: Option<String>,
    /// Replace the user's roles; repeat for more.
    #[arg(long = "role", value_parser = parse_role)]
    pub roles: Vec<Role>,
    #[arg(long, conflicts_with = "disable")]
    pub enable: bool,
    #[arg(long)]
    pub disable: bool,
    /// Read a new password from standard input.
    #[arg(long)]
    pub password: bool,
}

#[derive(Debug, Subcommand)]
pub enum TokenCommand {
    /// Create an API token for a user. The secret is shown once.
    Create {
        /// The user the token acts for.
        #[arg(long)]
        user: String,
        /// A name to recognise the token by, e.g. "Claude Desktop".
        #[arg(long)]
        name: String,
        /// read, draft, submit or admin.
        #[arg(long, value_parser = parse_scope, default_value = "read")]
        scope: Scope,
    },
    /// List tokens, optionally for one user.
    List {
        #[arg(long)]
        user: Option<String>,
    },
    /// Revoke a token by its id.
    Revoke { id: i64 },
}

fn parse_role(text: &str) -> Result<Role, String> {
    Role::from_key(text).ok_or_else(|| {
        format!(
            "unknown role {text:?}; use one of {}",
            Role::ALL.map(Role::key).join(", ")
        )
    })
}

fn parse_scope(text: &str) -> Result<Scope, String> {
    Scope::from_key(text)
        .ok_or_else(|| format!("unknown scope {text:?}; use read, draft, submit or admin"))
}

/// Exit codes. Clap itself exits with 2 for a usage error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Failed = 1,
    Invalid = 3,
    Conflict = 4,
    Missing = 5,
    /// The book is open in another process, usually a running server.
    BookInUse = 6,
}

#[derive(Debug)]
pub struct CliError {
    pub code: ExitCode,
    pub message: String,
}

impl CliError {
    fn new(code: ExitCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The error as printed: text, or `{"error": ..., "code": ...}` with `--json`.
    pub fn render(&self, as_json: bool) -> String {
        if as_json {
            json!({"error": self.message, "code": self.code as i32}).to_string()
        } else {
            format!("opentally: {}", self.message)
        }
    }
}

impl From<StoreError> for CliError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Invalid(message) => Self::new(ExitCode::Invalid, message),
            StoreError::Conflict(message) => Self::new(ExitCode::Conflict, message),
            StoreError::Missing(message) => Self::new(ExitCode::Missing, message),
            StoreError::Database(error) => database_error(&error.to_string()),
            StoreError::Unavailable => Self::new(ExitCode::Failed, "The book is unavailable."),
        }
    }
}

impl From<MigrationError> for CliError {
    fn from(error: MigrationError) -> Self {
        match error {
            MigrationError::Database(error) => database_error(&error.to_string()),
            other => Self::new(ExitCode::Failed, other.to_string()),
        }
    }
}

impl From<AuthError> for CliError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Invalid(message) => Self::new(ExitCode::Invalid, message),
            AuthError::Store(error) => error.into(),
            other => Self::new(ExitCode::Failed, format!("{other:?}")),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::new(ExitCode::Failed, error.to_string())
    }
}

fn database_error(text: &str) -> CliError {
    if text.contains("Could not set lock") || text.contains("Conflicting lock") {
        CliError::new(
            ExitCode::BookInUse,
            "The book is open in another program, probably a running OpenTally server. Stop it and try again.",
        )
    } else {
        CliError::new(ExitCode::Failed, format!("Database error: {text}"))
    }
}

/// The book path from `--db` or the environment.
pub fn book_path(cli: &Cli) -> PathBuf {
    cli.db
        .clone()
        .or_else(|| std::env::var_os("OPENTALLY_DB").map(PathBuf::from))
        .or_else(|| std::env::var_os("VOUCHER_DB").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("data/vouchers.duckdb"))
}

/// The result of a command: text for people and a JSON value for programs.
#[derive(Debug)]
pub struct Output {
    pub text: String,
    pub value: Value,
}

impl Output {
    fn new(text: impl Into<String>, value: impl Serialize) -> Self {
        Self {
            text: text.into(),
            value: serde_json::to_value(value).unwrap_or(Value::Null),
        }
    }

    pub fn render(&self, as_json: bool) -> String {
        if as_json {
            self.value.to_string()
        } else {
            self.text.clone()
        }
    }
}

/// Opens a book that must already exist, upgrading it if needed.
fn open_existing(path: &Path) -> Result<Store, CliError> {
    if !path.is_file() {
        return Err(CliError::new(
            ExitCode::Missing,
            format!("There is no book at {}.", path.display()),
        ));
    }
    migrate::backup_before_migrating(path)?;
    Ok(Store::open(path)?)
}

/// Runs a command other than `serve`. `input` supplies passwords.
pub fn run(cli: &Cli, command: &Command, input: &mut dyn BufRead) -> Result<Output, CliError> {
    let path = book_path(cli);
    match command {
        Command::Serve | Command::Mcp { .. } => unreachable!("handled by main"),
        Command::Migrate => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent)?;
            }
            let before = if path.exists() {
                migrate::file_version(&path)?
            } else {
                0
            };
            let backup = migrate::backup_before_migrating(&path)?;
            let store = Store::open(&path)?;
            let after = store.schema_version()?;
            let text = match &backup {
                _ if before == after => {
                    format!("The book is up to date at schema version {after}.")
                }
                Some(copy) => format!(
                    "Upgraded the book from schema version {before} to {after}. The old book is at {}.",
                    copy.display()
                ),
                None => format!("Created a new book at schema version {after}."),
            };
            Ok(Output::new(
                text,
                json!({"book": path, "from_version": before, "to_version": after, "backup": backup}),
            ))
        }
        Command::Info => {
            let store = open_existing(&path)?;
            let settings = serde_json::to_value(store.settings()?).unwrap_or(Value::Null);
            let version = store.schema_version()?;
            let users = store.user_count()?;
            let company = settings["company_name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            Ok(Output::new(
                format!(
                    "Book: {}\nCompany: {company}\nSchema version: {version}\nUsers: {users}",
                    path.display()
                ),
                json!({"book": path, "company": company, "schema_version": version,
                       "users": users, "settings": settings}),
            ))
        }
        Command::User(user) => run_user(&path, user, input),
        Command::Chart(ChartCommand::List) => {
            let charts = ot_core::accounts::charts();
            let text = charts
                .iter()
                .map(|c| format!("{:<4} {}", c.country_code, c.name))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(Output::new(text, &charts))
        }
        Command::Chart(ChartCommand::Apply { name }) => {
            let mut store = open_existing(&path)?;
            let applied = store.apply_chart(name)?;
            Ok(Output::new(
                format!(
                    "Applied {}: {} groups and {} ledgers created, {} ledgers placed in groups.",
                    applied.chart,
                    applied.groups_created,
                    applied.ledgers_created,
                    applied.ledgers_placed
                ),
                &applied,
            ))
        }
        Command::Item(ItemCommand::List { search }) => {
            let filter = ot_store::items::ItemFilter {
                group_id: None,
                q: search.clone(),
            };
            let rows = open_existing(&path)?.items(&filter)?;
            let text = rows.iter().map(item_text).collect::<Vec<_>>().join("\n");
            Ok(Output::new(text, &rows))
        }
        Command::Item(ItemCommand::Add {
            name,
            item_type,
            unit,
            rate,
            hsn,
        }) => {
            let item_type = serde_json::from_value(json!(item_type)).map_err(|_| {
                CliError::new(
                    ExitCode::Invalid,
                    format!("Unknown item type {item_type}; use product or service."),
                )
            })?;
            let input = ot_store::items::ItemInput {
                name: name.clone(),
                item_type,
                unit: unit.clone(),
                rate: rate.clone(),
                hsn_code: hsn.clone(),
                ..Default::default()
            };
            let item = open_existing(&path)?.add_item(input)?;
            Ok(Output::new(
                format!("Added {}", item_text(&item).trim_start()),
                &item,
            ))
        }
        Command::Item(ItemCommand::Delete { id }) => {
            open_existing(&path)?.delete_item(*id)?;
            Ok(Output::new(
                format!("Deleted item {id}."),
                json!({"deleted": id}),
            ))
        }
        Command::Unit(UnitCommand::List) => {
            let rows = open_existing(&path)?.units()?;
            let text = rows
                .iter()
                .map(|u| format!("{}{}", u.name, if u.is_whole { "  (whole)" } else { "" }))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(Output::new(text, &rows))
        }
        Command::Unit(UnitCommand::Add { name, whole }) => {
            let unit = open_existing(&path)?.add_unit(ot_store::items::NewUnit {
                name: name.clone(),
                is_whole: *whole,
            })?;
            Ok(Output::new(format!("Added unit {}.", unit.name), &unit))
        }
        Command::Party(PartyCommand::List { role, search }) => {
            let filter = ot_store::parties::PartyFilter {
                role: role.as_deref().map(party_role).transpose()?,
                q: search.clone(),
            };
            let rows = open_existing(&path)?.parties(&filter)?;
            let text = rows.iter().map(party_text).collect::<Vec<_>>().join("\n");
            Ok(Output::new(text, &rows))
        }
        Command::Party(PartyCommand::Add {
            name,
            role,
            gstin,
            email,
            phone,
        }) => {
            let input = ot_store::parties::PartyInput {
                name: name.clone(),
                role: party_role(role)?,
                gst_type: if gstin.is_some() {
                    ot_store::parties::GstType::RegisteredRegular
                } else {
                    ot_store::parties::GstType::Unregistered
                },
                gstin: gstin.clone(),
                email: email.clone(),
                phone: phone.clone(),
                ..Default::default()
            };
            let party = open_existing(&path)?.add_party(input)?;
            Ok(Output::new(
                format!("Added {}", party_text(&party).trim_start()),
                &party,
            ))
        }
        Command::Party(PartyCommand::Delete { id }) => {
            open_existing(&path)?.delete_party(*id)?;
            Ok(Output::new(
                format!("Deleted party {id}."),
                json!({"deleted": id}),
            ))
        }
        Command::Series(SeriesCommand::List) => {
            let rows = open_existing(&path)?.number_series()?;
            let text = rows.iter().map(series_text).collect::<Vec<_>>().join("\n");
            Ok(Output::new(text, &rows))
        }
        Command::Series(SeriesCommand::Add {
            prefix,
            reference_type,
            start,
            pad,
            default,
        }) => {
            let kind = ot_core::series::SeriesType::from_key(reference_type).ok_or_else(|| {
                CliError::new(
                    ExitCode::Invalid,
                    format!(
                        "Unknown document type {reference_type}; see `opentally series add --help`."
                    ),
                )
            })?;
            let row = open_existing(&path)?.add_number_series(ot_store::series::NewSeries {
                prefix: prefix.clone(),
                reference_type: kind,
                start: *start,
                pad_zeros: *pad,
                make_default: *default,
            })?;
            Ok(Output::new(format!("Added {}", series_text(&row)), &row))
        }
        Command::Series(SeriesCommand::Set {
            prefix,
            start,
            pad,
            default,
        }) => {
            let row = open_existing(&path)?.update_number_series(
                prefix,
                ot_store::series::SeriesUpdate {
                    start: *start,
                    pad_zeros: *pad,
                    make_default: default.then_some(true),
                },
            )?;
            Ok(Output::new(format!("Updated {}", series_text(&row)), &row))
        }
        Command::Series(SeriesCommand::Delete { prefix }) => {
            open_existing(&path)?.delete_number_series(prefix)?;
            Ok(Output::new(
                format!("Deleted number series {prefix}."),
                json!({"deleted": prefix}),
            ))
        }
        Command::Token(token) => run_token(&path, token),
        Command::Backup { to, force } => {
            let store = open_existing(&path)?;
            if to.exists() && !force {
                return Err(CliError::new(
                    ExitCode::Conflict,
                    format!("{} exists. Use --force to replace it.", to.display()),
                ));
            }
            store.checkpoint()?;
            drop(store);
            fs::copy(&path, to)?;
            let version = migrate::file_version(to)?;
            Ok(Output::new(
                format!(
                    "Backed up {} to {} (schema version {version}).",
                    path.display(),
                    to.display()
                ),
                json!({"book": path, "backup": to, "schema_version": version}),
            ))
        }
        Command::Restore { from, yes } => {
            if !yes {
                return Err(CliError::new(
                    ExitCode::Invalid,
                    "Restoring replaces the current book. Add --yes to confirm.",
                ));
            }
            if !from.is_file() {
                return Err(CliError::new(
                    ExitCode::Missing,
                    format!("There is no backup at {}.", from.display()),
                ));
            }
            // Refuses a file that is not an OpenTally book, or one from a newer version.
            let version = migrate::file_version(from)?;
            if version > migrate::MIGRATIONS.last().map_or(0, |m| m.version) {
                return Err(MigrationError::TooNew { version }.into());
            }
            let kept = if path.exists() {
                // Fails with "book in use" if a server has it open.
                Store::open(&path)?.checkpoint()?;
                let kept = PathBuf::from(format!("{}.before-restore", path.display()));
                fs::copy(&path, &kept)?;
                Some(kept)
            } else {
                None
            };
            let wal = PathBuf::from(format!("{}.wal", path.display()));
            let _ = fs::remove_file(wal);
            fs::copy(from, &path)?;
            let restored = Store::open(&path)?.schema_version()?;
            Ok(Output::new(
                format!(
                    "Restored {} from {}.{}",
                    path.display(),
                    from.display(),
                    kept.as_ref()
                        .map(|k| format!(" The previous book is at {}.", k.display()))
                        .unwrap_or_default()
                ),
                json!({"book": path, "from": from, "previous": kept, "schema_version": restored}),
            ))
        }
    }
}

fn read_password(input: &mut dyn BufRead, prompt: &str) -> Result<String, CliError> {
    if std::io::stdin().is_terminal() {
        eprint!("{prompt} (it will show as you type): ");
        let _ = std::io::stderr().flush();
    }
    let mut line = String::new();
    input.read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_owned();
    if password.is_empty() {
        return Err(CliError::new(
            ExitCode::Invalid,
            "Give the password on standard input, e.g. `printf '%s\\n' \"$PASSWORD\" | opentally user add ...`.",
        ));
    }
    Ok(password)
}

fn find_user(store: &Store, username: &str) -> Result<UserInfo, CliError> {
    store
        .credentials(username)?
        .map(|(user, _)| user)
        .ok_or_else(|| CliError::new(ExitCode::Missing, format!("There is no user {username}.")))
}

fn roles_text(roles: &[Role]) -> String {
    roles.iter().map(|r| r.key()).collect::<Vec<_>>().join(", ")
}

fn run_user(
    path: &Path,
    command: &UserCommand,
    input: &mut dyn BufRead,
) -> Result<Output, CliError> {
    match command {
        UserCommand::Add(args) => {
            let password = read_password(input, "Password")?;
            let username = args.username.trim().to_owned();
            if !ot_core::auth::valid_username(&username) {
                return Err(CliError::new(
                    ExitCode::Invalid,
                    "Use 1 to 64 letters, digits, dots, hyphens, underscores or @ for the user name.",
                ));
            }
            let record = NewUserRecord {
                username,
                full_name: args.full_name.trim().to_owned(),
                password_hash: auth::hash_password(&password)?,
                roles: args.roles.clone(),
            };
            let mut store = open_existing(path)?;
            let user = store.create_user(record)?;
            Ok(Output::new(
                format!(
                    "Added {} ({}) with roles {}.",
                    user.username,
                    user.full_name,
                    roles_text(&user.roles)
                ),
                &user,
            ))
        }
        UserCommand::List => {
            let store = open_existing(path)?;
            let users = store.users()?;
            let mut text = format!("{:<20} {:<28} {:<8} ROLES", "USER", "NAME", "ENABLED");
            for user in &users {
                text.push_str(&format!(
                    "\n{:<20} {:<28} {:<8} {}",
                    user.username,
                    user.full_name,
                    if user.enabled { "yes" } else { "no" },
                    roles_text(&user.roles)
                ));
            }
            if users.is_empty() {
                text = "The book has no users yet.".into();
            }
            Ok(Output::new(text, &users))
        }
        UserCommand::Set(args) => {
            let password_hash = if args.password {
                Some(auth::hash_password(&read_password(input, "New password")?)?)
            } else {
                None
            };
            let update = UserUpdate {
                full_name: args.full_name.as_ref().map(|n| n.trim().to_owned()),
                enabled: if args.enable {
                    Some(true)
                } else if args.disable {
                    Some(false)
                } else {
                    None
                },
                roles: (!args.roles.is_empty()).then(|| args.roles.clone()),
                password_hash,
            };
            let mut store = open_existing(path)?;
            let user = find_user(&store, &args.username)?;
            let user = store.update_user(user.id, update)?;
            Ok(Output::new(
                format!(
                    "Updated {}: {}, roles {}.",
                    user.username,
                    if user.enabled { "enabled" } else { "disabled" },
                    roles_text(&user.roles)
                ),
                &user,
            ))
        }
    }
}

fn run_token(path: &Path, command: &TokenCommand) -> Result<Output, CliError> {
    match command {
        TokenCommand::Create { user, name, scope } => {
            let mut store = open_existing(path)?;
            let owner = find_user(&store, user)?;
            if *scope == Scope::Admin && !owner.roles.contains(&Role::SystemManager) {
                return Err(CliError::new(
                    ExitCode::Invalid,
                    "Only a System Manager can have an admin token.",
                ));
            }
            let secret = format!("{}{}", auth::TOKEN_PREFIX, auth::random_hex(32)?);
            let info = store.create_token(owner.id, name, &auth::token_hash(&secret), *scope)?;
            let mut value = serde_json::to_value(&info).unwrap_or(Value::Null);
            value["secret"] = Value::String(secret.clone());
            Ok(Output {
                text: format!(
                    "Created token {} \"{}\" for {} with scope {}.\n{secret}\nKeep this secret safe; it is not shown again.",
                    info.id,
                    info.name,
                    info.username,
                    info.scope.key()
                ),
                value,
            })
        }
        TokenCommand::List { user } => {
            let store = open_existing(path)?;
            let owner = user.as_deref().map(|u| find_user(&store, u)).transpose()?;
            let tokens = store.tokens(owner.map(|u| u.id))?;
            let mut text = format!(
                "{:<5} {:<16} {:<24} {:<7} STATE",
                "ID", "USER", "NAME", "SCOPE"
            );
            for token in &tokens {
                text.push_str(&format!(
                    "\n{:<5} {:<16} {:<24} {:<7} {}",
                    token.id,
                    token.username,
                    token.name,
                    token.scope.key(),
                    if token.revoked { "revoked" } else { "active" }
                ));
            }
            if tokens.is_empty() {
                text = "There are no tokens.".into();
            }
            Ok(Output::new(text, &tokens))
        }
        TokenCommand::Revoke { id } => {
            let mut store = open_existing(path)?;
            let token = store.revoke_token(*id, None)?;
            Ok(Output::new(
                format!("Revoked token {} \"{}\".", token.id, token.name),
                &token,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn cli(dir: &Path, args: &[&str]) -> Cli {
        let book = dir.join("book.duckdb");
        let mut all = vec!["opentally", "--db", book.to_str().unwrap()];
        all.extend_from_slice(args);
        Cli::parse_from(all)
    }

    fn exec(dir: &Path, args: &[&str], stdin: &str) -> Result<Output, CliError> {
        let cli = cli(dir, args);
        let command = cli.command.as_ref().unwrap();
        run(&cli, command, &mut Cursor::new(stdin.as_bytes().to_vec()))
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ot-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn migrate_creates_then_reports_up_to_date() {
        let dir = temp("migrate");
        let created = exec(&dir, &["migrate"], "").unwrap();
        assert!(
            created.text.starts_with("Created a new book"),
            "{}",
            created.text
        );
        let again = exec(&dir, &["--json", "migrate"], "").unwrap();
        assert_eq!(again.value["from_version"], again.value["to_version"]);
        let info = exec(&dir, &["info"], "").unwrap();
        assert!(info.text.contains("Users: 0"), "{}", info.text);
    }

    #[test]
    fn missing_book_and_bad_input_have_their_own_exit_codes() {
        let dir = temp("codes");
        assert_eq!(
            exec(&dir, &["info"], "").unwrap_err().code,
            ExitCode::Missing
        );
        exec(&dir, &["migrate"], "").unwrap();
        let no_password = exec(
            &dir,
            &[
                "user",
                "add",
                "asha",
                "--name",
                "Asha",
                "--role",
                "system_manager",
            ],
            "",
        );
        assert_eq!(no_password.unwrap_err().code, ExitCode::Invalid);
        let short = exec(
            &dir,
            &[
                "user",
                "add",
                "asha",
                "--name",
                "Asha",
                "--role",
                "system_manager",
            ],
            "short\n",
        );
        assert_eq!(short.unwrap_err().code, ExitCode::Invalid);
        assert!(
            Cli::try_parse_from([
                "opentally",
                "user",
                "add",
                "x",
                "--name",
                "X",
                "--role",
                "boss"
            ])
            .is_err()
        );
        let error = CliError::new(ExitCode::Conflict, "taken");
        assert_eq!(error.render(true), r#"{"code":4,"error":"taken"}"#);
    }

    #[test]
    fn users_and_tokens_round_trip() {
        let dir = temp("users");
        exec(&dir, &["migrate"], "").unwrap();
        let added = exec(
            &dir,
            &[
                "--json",
                "user",
                "add",
                "asha",
                "--name",
                "Asha Patel",
                "--role",
                "system_manager",
            ],
            "correct horse battery\n",
        )
        .unwrap();
        assert_eq!(added.value["roles"], json!(["system_manager"]));
        let duplicate = exec(
            &dir,
            &["user", "add", "asha", "--name", "A", "--role", "viewer"],
            "correct horse battery\n",
        );
        assert_eq!(duplicate.unwrap_err().code, ExitCode::Conflict);
        exec(
            &dir,
            &[
                "user",
                "add",
                "ravi",
                "--name",
                "Ravi",
                "--role",
                "books_user",
            ],
            "another long secret\n",
        )
        .unwrap();
        let list = exec(&dir, &["user", "list"], "").unwrap();
        assert!(
            list.text.contains("asha") && list.text.contains("books_user"),
            "{}",
            list.text
        );

        // The last System Manager cannot be disabled.
        let disable = exec(&dir, &["user", "set", "asha", "--disable"], "");
        assert_eq!(disable.unwrap_err().code, ExitCode::Conflict);
        let renamed = exec(
            &dir,
            &[
                "user",
                "set",
                "ravi",
                "--name",
                "Ravi Shah",
                "--role",
                "viewer",
            ],
            "",
        )
        .unwrap();
        assert!(renamed.text.contains("viewer"));

        let admin_for_viewer = exec(
            &dir,
            &[
                "token", "create", "--user", "ravi", "--name", "X", "--scope", "admin",
            ],
            "",
        );
        assert_eq!(admin_for_viewer.unwrap_err().code, ExitCode::Invalid);
        let token = exec(
            &dir,
            &[
                "--json", "token", "create", "--user", "asha", "--name", "Claude", "--scope",
                "draft",
            ],
            "",
        )
        .unwrap();
        let secret = token.value["secret"].as_str().unwrap().to_owned();
        assert!(secret.starts_with("ot_") && secret.len() == 67);
        let store = Store::open(&dir.join("book.duckdb")).unwrap();
        let (user, scope) = store.token(&auth::token_hash(&secret)).unwrap().unwrap();
        assert_eq!((user.username.as_str(), scope), ("asha", Scope::Draft));
        drop(store);
        let id = token.value["id"].as_i64().unwrap().to_string();
        assert!(
            exec(&dir, &["token", "revoke", &id], "")
                .unwrap()
                .text
                .starts_with("Revoked")
        );
        let tokens = exec(&dir, &["--json", "token", "list", "--user", "asha"], "").unwrap();
        assert_eq!(tokens.value[0]["revoked"], true);
    }

    #[test]
    fn backup_and_restore_keep_the_previous_book() {
        let dir = temp("backup");
        exec(&dir, &["migrate"], "").unwrap();
        exec(
            &dir,
            &[
                "user",
                "add",
                "asha",
                "--name",
                "Asha",
                "--role",
                "system_manager",
            ],
            "correct horse battery\n",
        )
        .unwrap();
        let copy = dir.join("copy.duckdb");
        exec(&dir, &["backup", copy.to_str().unwrap()], "").unwrap();
        let again = exec(&dir, &["backup", copy.to_str().unwrap()], "");
        assert_eq!(again.unwrap_err().code, ExitCode::Conflict);

        // A change after the backup is undone by restoring it.
        exec(
            &dir,
            &["user", "add", "ravi", "--name", "Ravi", "--role", "viewer"],
            "another long secret\n",
        )
        .unwrap();
        let unconfirmed = exec(&dir, &["restore", copy.to_str().unwrap()], "");
        assert_eq!(unconfirmed.unwrap_err().code, ExitCode::Invalid);
        let restored = exec(&dir, &["restore", copy.to_str().unwrap(), "--yes"], "").unwrap();
        assert!(
            restored.text.contains("before-restore"),
            "{}",
            restored.text
        );
        let users = exec(&dir, &["--json", "user", "list"], "").unwrap();
        assert_eq!(users.value.as_array().unwrap().len(), 1);
        assert!(dir.join("book.duckdb.before-restore").exists());

        let not_a_book = dir.join("notes.txt");
        fs::write(&not_a_book, "hello").unwrap();
        assert!(
            exec(
                &dir,
                &["restore", not_a_book.to_str().unwrap(), "--yes"],
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn chart_templates_are_listed_and_applied() {
        let dir = temp("chart");
        exec(&dir, &["migrate"], "").unwrap();
        let list = exec(&dir, &["--json", "chart", "list"], "").unwrap();
        assert_eq!(list.value.as_array().unwrap().len(), 14);
        let applied = exec(
            &dir,
            &["chart", "apply", "Tally (India) - Reserved Groups"],
            "",
        )
        .unwrap();
        assert!(applied.text.contains("28 groups"), "{}", applied.text);
        let unknown = exec(&dir, &["chart", "apply", "Atlantis"], "");
        assert_eq!(unknown.unwrap_err().code, ExitCode::Invalid);
    }

    #[test]
    fn number_series_are_managed_from_the_command_line() {
        let dir = temp("series");
        exec(&dir, &["migrate"], "").unwrap();
        let list = exec(&dir, &["--json", "series", "list"], "").unwrap();
        assert_eq!(list.value.as_array().unwrap().len(), 9);
        let added = exec(
            &dir,
            &[
                "series",
                "add",
                "GST-26-",
                "--type",
                "sales_invoice",
                "--start",
                "1",
                "--pad",
                "3",
                "--default",
            ],
            "",
        )
        .unwrap();
        assert!(added.text.contains("next GST-26-001"), "{}", added.text);
        assert!(added.text.contains("(default)"), "{}", added.text);
        let set = exec(&dir, &["series", "set", "GST-26-", "--start", "50"], "").unwrap();
        assert!(set.text.contains("next GST-26-050"), "{}", set.text);
        exec(&dir, &["series", "delete", "SINV-"], "").unwrap();
        let missing = exec(&dir, &["series", "delete", "SINV-"], "");
        assert_eq!(missing.unwrap_err().code, ExitCode::Missing);
        let unknown = exec(&dir, &["series", "add", "X-", "--type", "invoice"], "");
        assert_eq!(unknown.unwrap_err().code, ExitCode::Invalid);
    }

    #[test]
    fn parties_are_managed_from_the_command_line() {
        let dir = temp("parties");
        exec(&dir, &["migrate"], "").unwrap();
        let added = exec(
            &dir,
            &[
                "--json",
                "party",
                "add",
                "Asha Stores",
                "--role",
                "customer",
                "--gstin",
                "24aapfu0939f1zv",
            ],
            "",
        )
        .unwrap();
        assert_eq!(added.value["gstin"], "24AAPFU0939F1ZV");
        let id = added.value["id"].as_i64().unwrap();
        exec(
            &dir,
            &["party", "add", "Kiran Mills", "--role", "supplier"],
            "",
        )
        .unwrap();
        let customers = exec(&dir, &["--json", "party", "list", "--role", "customer"], "").unwrap();
        assert_eq!(customers.value.as_array().unwrap().len(), 1);
        let bad = exec(&dir, &["party", "add", "X", "--gstin", "invalid"], "");
        assert_eq!(bad.unwrap_err().code, ExitCode::Invalid);
        let role = exec(&dir, &["party", "add", "Y", "--role", "vendor"], "");
        assert_eq!(role.unwrap_err().code, ExitCode::Invalid);
        let duplicate = exec(&dir, &["party", "add", "asha stores"], "");
        assert_eq!(duplicate.unwrap_err().code, ExitCode::Conflict);
        exec(&dir, &["party", "delete", &id.to_string()], "").unwrap();
        let gone = exec(&dir, &["party", "delete", &id.to_string()], "");
        assert_eq!(gone.unwrap_err().code, ExitCode::Missing);
    }

    #[test]
    fn items_and_units_are_managed_from_the_command_line() {
        let dir = temp("items");
        exec(&dir, &["migrate"], "").unwrap();
        // A book whose company is not set up has no Sales ledger to default to.
        let no_account = exec(&dir, &["item", "add", "Rice"], "");
        assert_eq!(no_account.unwrap_err().code, ExitCode::Invalid);
        let book = dir.join("book.duckdb");
        {
            let mut store = Store::open(&book).unwrap();
            let setup: ot_store::chart::CompanySetup = serde_json::from_value(json!({
                "company_name": "Shree Traders", "country": "India", "bank_name": "HDFC Bank"
            }))
            .unwrap();
            store
                .setup_company(setup, ot_core::setup::today_utc())
                .unwrap();
        }
        exec(&dir, &["unit", "add", "Bag", "--whole"], "").unwrap();
        let units = exec(&dir, &["--json", "unit", "list"], "").unwrap();
        assert_eq!(units.value.as_array().unwrap().len(), 7);
        let added = exec(
            &dir,
            &[
                "--json",
                "item",
                "add",
                "Basmati Rice",
                "--unit",
                "Kg",
                "--rate",
                "125.5",
                "--hsn",
                "1006",
            ],
            "",
        )
        .unwrap();
        assert_eq!(added.value["rate"], "125.50");
        let id = added.value["id"].as_i64().unwrap().to_string();
        let found = exec(&dir, &["--json", "item", "list", "--search", "rice"], "").unwrap();
        assert_eq!(found.value.as_array().unwrap().len(), 1);
        let bad = exec(&dir, &["item", "add", "Dal", "--hsn", "12A4"], "");
        assert_eq!(bad.unwrap_err().code, ExitCode::Invalid);
        let kind = exec(&dir, &["item", "add", "Dal", "--type", "good"], "");
        assert_eq!(kind.unwrap_err().code, ExitCode::Invalid);
        exec(&dir, &["item", "delete", &id], "").unwrap();
    }

    #[test]
    fn a_book_in_use_is_reported_clearly() {
        let dir = temp("locked");
        exec(&dir, &["migrate"], "").unwrap();
        // DuckDB locks the file per process; another process holding it reports this text.
        let error = database_error(
            "IO Error: Could not set lock on file \"book.duckdb\": Conflicting lock is held",
        );
        assert_eq!(error.code, ExitCode::BookInUse);
    }
}
