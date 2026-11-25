//! The slash commands: `/port` (its definition and reading its options) and the porter's control
//! commands, used in a port's channel (`/status`, `/stop`, `/resume`, `/retry`, `/note`).

use craft_anything_discord::run::Target;
use serenity::all::{CommandInteraction, CommandOptionType, CreateCommand, CreateCommandOption, ResolvedValue};

pub const PORT: &str = "port";
pub const STATUS: &str = "status";
pub const STOP: &str = "stop";
pub const RESUME: &str = "resume";
pub const RETRY: &str = "retry";
pub const NOTE: &str = "note";
/// The control commands, each with its description.
pub const CONTROL: [(&str, &str); 5] = [
    (STATUS, "This port's stage, features, place in the queue, running time and cost"),
    (STOP, "Stop this port's running build cleanly, or take it out of the queue"),
    (RESUME, "Carry on this port's stopped, paused, failed or interrupted run where it stopped"),
    (RETRY, "Run this port's request again from the start, in the same project folder"),
    (NOTE, "Leave the builder a note: a running build reads it at its next step"),
];
const MAX_NAME_LENGTH: u16 = 80;
/// The longest `/note`, the same as a tester's "What went wrong?".
const MAX_NOTE_LENGTH: u16 = craft_anything_discord::review::WHAT_LIMIT;

/// Every command the bot registers in the guild.
pub fn commands() -> Vec<CreateCommand> {
    let mut all = vec![port_command()];
    for (name, description) in CONTROL {
        let mut command = CreateCommand::new(name).description(description);
        if name == NOTE {
            command = command
                .add_option(CreateCommandOption::new(CommandOptionType::String, "text", "The note for the builder").required(true).max_length(MAX_NOTE_LENGTH));
        }
        all.push(command);
    }
    all
}

/// `/note`'s text, trimmed; `None` when blank.
pub fn note_text(cmd: &CommandInteraction) -> Option<String> {
    cmd.data.options().into_iter().find_map(|o| match (o.name, o.value) {
        ("text", ResolvedValue::String(s)) => Some(s.trim().to_owned()).filter(|t| !t.is_empty()),
        _ => None,
    })
}

/// `/port apk:<file> link:<url> target:<unity|cocos|app> name:<text>`; one of `apk` or `link`
/// is required, which Discord can't express, so [`craft_anything_discord::intake::Source::choose`] checks it.
pub fn port_command() -> CreateCommand {
    let target =
        Target::CHOICES.iter().fold(CreateCommandOption::new(CommandOptionType::String, "target", "What to rebuild it as (default: Unity)"), |option, t| {
            option.add_string_choice(t.label(), t.choice())
        });
    CreateCommand::new(PORT)
        .description("Port an Android app: rebuild it as a Unity, Cocos Creator or mobile app project")
        .add_option(CreateCommandOption::new(CommandOptionType::Attachment, "apk", "The .apk, .xapk, .apks or .aab file"))
        .add_option(CreateCommandOption::new(CommandOptionType::String, "link", "A download link to the file, instead of attaching it"))
        .add_option(target)
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "name", "The app's name for its channel (default: from the file)").max_length(MAX_NAME_LENGTH),
        )
}

/// The options as given; nothing is checked here.
#[derive(Debug, Default)]
pub struct PortOptions {
    /// URL, file name and size of the attachment.
    pub attachment: Option<(String, String, u64)>,
    pub link: Option<String>,
    pub target: Target,
    pub name: Option<String>,
}

pub fn read_options(cmd: &CommandInteraction) -> PortOptions {
    let mut options = PortOptions::default();
    for option in cmd.data.options() {
        match (option.name, option.value) {
            ("apk", ResolvedValue::Attachment(a)) => options.attachment = Some((a.url.clone(), a.filename.clone(), u64::from(a.size))),
            ("link", ResolvedValue::String(s)) => options.link = Some(s.to_owned()),
            ("target", ResolvedValue::String(s)) => options.target = Target::from_choice(s).unwrap_or_default(),
            ("name", ResolvedValue::String(s)) => options.name = Some(s.trim().to_owned()).filter(|n| !n.is_empty()),
            _ => {}
        }
    }
    options
}
