use super::Destination;
use crate::{
    GatewayServer,
    traits::{
        IncomingMessengerEvent, InlineButton, MessengerPlatform, MessengerTransport,
        OutboundMessage,
    },
};
use async_trait::async_trait;
use serenity::all::{
    Channel, ChannelId, ChannelType, Command, CommandDataOptionValue, CommandOptionType, Context,
    CreateActionRow, CreateAllowedMentions, CreateButton, CreateCommand, CreateCommandOption,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditMessage,
    EventHandler, GatewayIntents, Http, Interaction, MessageId, Ready, UserId,
};
use std::sync::atomic::{AtomicBool, Ordering};
use teloxide::{
    dispatching::{Dispatcher, UpdateFilterExt},
    dptree,
    payloads::{EditMessageTextSetters, SendMessageSetters},
    prelude::{Bot, Requester},
    types::{
        BotCommand, CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, ThreadId,
        Update,
    },
};

const COMMANDS: [(&str, &str, Option<(&str, &str, bool)>); 12] = [
    (
        "pair",
        "Link your WebUI account in a direct message",
        Some(("code", "WebUI pairing code", true)),
    ),
    (
        "project",
        "Select a project in a direct message",
        Some(("project", "Project ID or slug", false)),
    ),
    (
        "session",
        "Select a project session in a direct message",
        Some(("id", "Session ID", false)),
    ),
    (
        "new",
        "Create a session in a direct message",
        Some((
            "options",
            "Session title or interactive followed by a title",
            false,
        )),
    ),
    (
        "cli",
        "Select a CLI provider in a direct message",
        Some(("provider", "Provider name", false)),
    ),
    (
        "account",
        "Select a CLI account in a direct message",
        Some(("id", "CLI account ID", false)),
    ),
    ("kanban", "Show the project Kanban board", None),
    ("stop", "Stop the current session turn", None),
    (
        "bind",
        "Link this channel to a project as an administrator",
        Some(("project", "Project ID or slug", true)),
    ),
    (
        "notify",
        "Configure Kanban notifications",
        Some(("setting", "on or off", false)),
    ),
    ("help", "Show the gateway commands", None),
    ("start", "Show the gateway commands", None),
];

pub struct BotTransport {
    telegram: Option<Bot>,
    discord: Option<DiscordConnection>,
}

struct DiscordConnection {
    token: String,
    http: Http,
}

impl BotTransport {
    pub fn new(telegram_token: Option<String>, discord_token: Option<String>) -> Self {
        Self {
            telegram: telegram_token
                .filter(|token| !token.trim().is_empty())
                .map(Bot::new),
            discord: discord_token
                .filter(|token| !token.trim().is_empty())
                .map(|token| DiscordConnection {
                    http: Http::new(&token),
                    token,
                }),
        }
    }

    pub fn start(&self, server: &GatewayServer) {
        if let Some(bot) = &self.telegram {
            let server = server.clone();
            let bot = bot.clone();
            tokio::spawn(async move {
                let commands = COMMANDS
                    .into_iter()
                    .map(|(name, description, _)| BotCommand::new(name, description));
                if bot.set_my_commands(commands).await.is_err() {
                    tracing::warn!("Telegram bot command registration failed");
                }

                let handler = dptree::entry()
                    .branch(Update::filter_message().endpoint(telegram_message))
                    .branch(Update::filter_callback_query().endpoint(telegram_callback));
                Dispatcher::builder(bot, handler)
                    .dependencies(dptree::deps![server])
                    .build()
                    .dispatch()
                    .await;
            });
        }
        if let Some(discord) = &self.discord {
            let server = server.clone();
            let token = discord.token.clone();
            tokio::spawn(async move {
                let intents = GatewayIntents::GUILDS
                    | GatewayIntents::GUILD_MESSAGES
                    | GatewayIntents::DIRECT_MESSAGES
                    | GatewayIntents::MESSAGE_CONTENT;
                let handler = DiscordHandler {
                    server,
                    registered: AtomicBool::new(false),
                };
                match serenity::Client::builder(&token, intents)
                    .event_handler(handler)
                    .await
                {
                    Ok(mut client) => {
                        if client.start().await.is_err() {
                            tracing::error!("Discord gateway disconnected");
                        }
                    }
                    Err(_) => tracing::error!("Discord bot initialization failed"),
                }
            });
        }
    }
}

fn telegram_markup(rows: Vec<Vec<InlineButton>>) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(rows.into_iter().map(|row| {
        row.into_iter().map(|button| {
            InlineKeyboardButton::callback(
                button.label.chars().take(80).collect::<String>(),
                button.callback_data,
            )
        })
    }))
}

fn discord_components(rows: Vec<Vec<InlineButton>>) -> Vec<CreateActionRow> {
    let buttons: Vec<_> = rows.into_iter().flatten().collect();
    buttons
        .chunks(5)
        .map(|row| {
            CreateActionRow::Buttons(
                row.iter()
                    .map(|button| {
                        CreateButton::new(&button.callback_data)
                            .label(button.label.chars().take(80).collect::<String>())
                            .style(serenity::all::ButtonStyle::Secondary)
                    })
                    .collect(),
            )
        })
        .collect()
}

fn split_text(text: &str, max: usize) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some((boundary, _)) = rest.char_indices().nth(max) {
        let candidate = &rest[..boundary];
        let end = candidate
            .rfind('\n')
            .filter(|i| *i > boundary / 2)
            .map_or(boundary, |i| i + 1);
        let (piece, tail) = rest.split_at(end);
        pieces.push(piece);
        rest = tail;
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

#[async_trait]
impl MessengerTransport for BotTransport {
    async fn send_message(&self, message: OutboundMessage) -> Result<String, String> {
        let mut message_id = None;
        let max = match message.platform {
            MessengerPlatform::Telegram => 3500,
            MessengerPlatform::Discord => 1800,
        };
        let parts = split_text(&message.text, max);
        for (index, text) in parts.into_iter().enumerate() {
            let buttons = if index == 0 {
                message.buttons.clone()
            } else {
                Vec::new()
            };
            let id = match message.platform {
                MessengerPlatform::Telegram => {
                    let bot = self.telegram.as_ref().ok_or("Telegram bot is disabled")?;
                    let chat = ChatId(
                        message
                            .chat_id
                            .parse()
                            .map_err(|_| "invalid Telegram chat ID")?,
                    );
                    let mut request = bot
                        .send_message(chat, text)
                        .reply_markup(telegram_markup(buttons));
                    if let Some(thread) = &message.thread_id {
                        let id = thread.parse().map_err(|_| "invalid Telegram topic ID")?;
                        request =
                            request.message_thread_id(ThreadId(teloxide::types::MessageId(id)));
                    }
                    request
                        .await
                        .map_err(|_| "Telegram rejected the message")?
                        .id
                        .0
                        .to_string()
                }
                MessengerPlatform::Discord => {
                    let discord = self.discord.as_ref().ok_or("Discord bot is disabled")?;
                    let target = message.thread_id.as_deref().unwrap_or(&message.chat_id);
                    let channel = discord_target(&discord.http, target).await?;
                    channel
                        .send_message(
                            &discord.http,
                            CreateMessage::new()
                                .content(text)
                                .allowed_mentions(
                                    CreateAllowedMentions::new().empty_users().empty_roles(),
                                )
                                .components(discord_components(buttons)),
                        )
                        .await
                        .map_err(|_| "Discord rejected the message")?
                        .id
                        .to_string()
                }
            };
            message_id.get_or_insert(id);
        }
        message_id.ok_or_else(|| "cannot send an empty messenger message".to_owned())
    }

    async fn edit_message(
        &self,
        platform: MessengerPlatform,
        chat_id: String,
        message_id: String,
        text: String,
        buttons: Vec<Vec<InlineButton>>,
    ) -> Result<(), String> {
        match platform {
            MessengerPlatform::Telegram => {
                let bot = self.telegram.as_ref().ok_or("Telegram bot is disabled")?;
                let chat = ChatId(chat_id.parse().map_err(|_| "invalid Telegram chat ID")?);
                let id = teloxide::types::MessageId(
                    message_id
                        .parse()
                        .map_err(|_| "invalid Telegram message ID")?,
                );
                bot.edit_message_text(chat, id, text)
                    .reply_markup(telegram_markup(buttons))
                    .await
                    .map_err(|_| "Telegram rejected the message edit")?;
            }
            MessengerPlatform::Discord => {
                let discord = self.discord.as_ref().ok_or("Discord bot is disabled")?;
                let channel = discord_target(&discord.http, &chat_id).await?;
                let id = MessageId::new(
                    message_id
                        .parse()
                        .map_err(|_| "invalid Discord message ID")?,
                );
                channel
                    .edit_message(
                        &discord.http,
                        id,
                        EditMessage::new()
                            .content(text)
                            .allowed_mentions(
                                CreateAllowedMentions::new().empty_users().empty_roles(),
                            )
                            .components(discord_components(buttons)),
                    )
                    .await
                    .map_err(|_| "Discord rejected the message edit")?;
            }
        }
        Ok(())
    }
}

async fn discord_target(http: &Http, target: &str) -> Result<ChannelId, String> {
    if let Some(user) = target.strip_prefix("user:") {
        let id = user.parse().map_err(|_| "invalid Discord user ID")?;
        let channel = UserId::new(id)
            .create_dm_channel(http)
            .await
            .map_err(|_| "could not open Discord direct message")?;
        return Ok(channel.id);
    }
    let id = target.parse().map_err(|_| "invalid Discord channel ID")?;
    Ok(ChannelId::new(id))
}

async fn listen_to_message(server: &GatewayServer, event: &IncomingMessengerEvent) -> bool {
    if event.is_private
        || event
            .text
            .as_deref()
            .is_some_and(|text| text.starts_with('/'))
    {
        return true;
    }
    if event.thread_id.is_none() {
        return false;
    }
    let Ok(mut conn) = server.db().acquire().await else {
        return false;
    };
    super::channel_project(&mut conn, &Destination::from(event))
        .await
        .is_ok_and(|project| project.is_some())
}

async fn telegram_message(
    server: GatewayServer,
    message: teloxide::types::Message,
    bot: teloxide::types::Me,
) -> Result<(), teloxide::RequestError> {
    let (Some(author), Some(text)) = (&message.from, message.text()) else {
        return Ok(());
    };
    if author.is_bot {
        return Ok(());
    }
    let command = text.split_whitespace().next().unwrap_or_default();
    if command.starts_with('/')
        && command
            .split_once('@')
            .is_some_and(|(_, username)| !username.eq_ignore_ascii_case(bot.username()))
    {
        return Ok(());
    }
    let event = telegram_event(&message, author.id.0, Some(text.into()), None);
    if listen_to_message(&server, &event).await {
        let _ = server.handle_messenger_event(event).await;
    }
    Ok(())
}

async fn telegram_callback(
    bot: Bot,
    server: GatewayServer,
    callback: CallbackQuery,
) -> Result<(), teloxide::RequestError> {
    if let (Some(message), Some(data)) = (callback.regular_message(), callback.data.clone()) {
        // Acknowledge quickly so the platform stops displaying its loading indicator.
        let _ = bot.answer_callback_query(callback.id.clone()).await;
        let event = telegram_event(message, callback.from.id.0, None, Some(data));
        let _ = server.handle_messenger_event(event).await;
    }
    Ok(())
}

fn telegram_event(
    message: &teloxide::types::Message,
    user: u64,
    text: Option<String>,
    callback_data: Option<String>,
) -> IncomingMessengerEvent {
    IncomingMessengerEvent {
        platform: MessengerPlatform::Telegram,
        chat_id: message.chat.id.0.to_string(),
        thread_id: message
            .thread_id
            .filter(|_| message.is_topic_message)
            .map(|id| id.0.0.to_string()),
        external_user_id: user.to_string(),
        is_private: message.chat.is_private(),
        text,
        callback_data,
    }
}

struct DiscordHandler {
    server: GatewayServer,
    registered: AtomicBool,
}

#[serenity::async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, ctx: Context, _: Ready) {
        if self.registered.swap(true, Ordering::Relaxed) {
            return;
        }
        let commands = COMMANDS
            .into_iter()
            .map(|(name, description, argument)| {
                let mut command = CreateCommand::new(name).description(description);
                if let Some((name, description, required)) = argument {
                    command = command.add_option(
                        CreateCommandOption::new(CommandOptionType::String, name, description)
                            .required(required),
                    );
                }
                command
            })
            .collect();
        if Command::set_global_commands(&ctx.http, commands)
            .await
            .is_err()
        {
            self.registered.store(false, Ordering::Relaxed);
            tracing::warn!("Discord slash command registration failed");
        }
    }

    async fn message(&self, ctx: Context, message: serenity::all::Message) {
        if message.author.bot || message.webhook_id.is_some() || message.content.trim().is_empty() {
            return;
        }
        let Some((chat_id, thread_id)) =
            discord_destination(&ctx.http, message.channel_id, message.guild_id.is_none()).await
        else {
            return;
        };
        let event = IncomingMessengerEvent {
            platform: MessengerPlatform::Discord,
            chat_id,
            thread_id,
            external_user_id: message.author.id.to_string(),
            is_private: message.guild_id.is_none(),
            text: Some(message.content),
            callback_data: None,
        };
        if listen_to_message(&self.server, &event).await {
            let _ = self.server.handle_messenger_event(event).await;
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        let (channel, external_user_id, private, text, callback_data) = match interaction {
            Interaction::Command(command) => {
                let name = &command.data.name;
                if !COMMANDS.iter().any(|(command, _, _)| command == name) {
                    return;
                }
                let argument =
                    command
                        .data
                        .options
                        .first()
                        .and_then(|option| match &option.value {
                            CommandDataOptionValue::String(value) => Some(value.as_str()),
                            _ => None,
                        });
                let text = match argument {
                    Some(value) => format!("/{name} {value}"),
                    None => format!("/{name}"),
                };
                let response = CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Gateway command received.")
                        .ephemeral(true),
                );
                let _ = command.create_response(&ctx.http, response).await;
                (
                    command.channel_id,
                    command.user.id.to_string(),
                    command.guild_id.is_none(),
                    Some(text),
                    None,
                )
            }
            Interaction::Component(callback) => {
                let _ = callback
                    .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
                    .await;
                (
                    callback.channel_id,
                    callback.user.id.to_string(),
                    callback.guild_id.is_none(),
                    None,
                    Some(callback.data.custom_id),
                )
            }
            _ => return,
        };
        let Some((chat_id, thread_id)) = discord_destination(&ctx.http, channel, private).await
        else {
            return;
        };
        let _ = self
            .server
            .handle_messenger_event(IncomingMessengerEvent {
                platform: MessengerPlatform::Discord,
                chat_id,
                thread_id,
                external_user_id,
                is_private: private,
                text,
                callback_data,
            })
            .await;
    }
}

async fn discord_destination(
    http: &Http,
    channel: ChannelId,
    private: bool,
) -> Option<(String, Option<String>)> {
    if private {
        return Some((channel.to_string(), None));
    }
    let Channel::Guild(resolved) = channel.to_channel(http).await.ok()? else {
        return None;
    };
    match resolved.kind {
        ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::NewsThread => {
            Some((resolved.parent_id?.to_string(), Some(channel.to_string())))
        }
        _ => Some((channel.to_string(), None)),
    }
}
