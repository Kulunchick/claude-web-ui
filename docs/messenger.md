# Telegram and Discord

Set `CLAUDE_WEB_UI_TELEGRAM_TOKEN` and/or `CLAUDE_WEB_UI_DISCORD_TOKEN` in the server environment. The gateway starts each configured bot at launch. The Discord application needs the Message Content intent and permission to read and send messages in its channels and threads. To receive ordinary messages in Telegram forum topics, disable the bot's group privacy mode in BotFather.

Select **Pair Telegram or Discord** in the signed-in dashboard. Send `/pair <code>` in a direct message to the bot within five minutes. Generate a separate code for each platform.

In a DM, use `/project` to pick a project and `/session` to select an existing session, or use `/new`. Use `/new interactive` to create a session that pauses for approval. `/cli` and `/account` change the project's CLI selection. Switching accounts in the same provider preserves the session. Changing providers starts a separate native session if the current session already has turns.

An administrator can run `/bind <project ID or slug>` in a Telegram forum group or Discord parent channel whose audience may see that project's output. A forum topic or Discord thread gets its own persistent session when someone sends its first prompt. The bound channel receives updates when cards enter Review or Blocked. Use `/notify on` inside a topic or thread to send updates there, `/notify off` in the channel to disable updates, or `/notify on` in a DM to subscribe privately to the selected project.

Administrators can revoke pairing with an authenticated `DELETE /api/admin/users/{user_id}/messenger/{platform}` request. The platform is `telegram` or `discord`.
