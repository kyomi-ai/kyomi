# Optional Integrations

Kyomi works out of the box with just an LLM provider and a database. These integrations add additional capabilities but are not required.

## SMTP (Email)

Email is used for account verification, password reset, and watch alert notifications.

### When to configure

- You want users to verify their email addresses during signup
- You want password reset emails to work
- You want watch alerts delivered via email (Enterprise edition)

### Without SMTP

When SMTP is not configured:
- Email verification is skipped -- users can sign up and use Kyomi immediately
- Password reset via email is unavailable (users must use passkeys or contact an admin)
- Watch email alerts are disabled (in-app and push notifications still work)

### Configuration

```env
SMTP_HOST=smtp.example.com
SMTP_PORT=587
SMTP_USER=your-smtp-username
SMTP_PASSWORD=your-smtp-password
SMTP_FROM_EMAIL=noreply@yourdomain.com
SMTP_FROM_NAME=Kyomi
```

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `SMTP_HOST` | Yes | -- | SMTP server hostname |
| `SMTP_PORT` | No | `587` | SMTP port (587 for STARTTLS, 465 for SMTPS) |
| `SMTP_USER` | Yes | -- | SMTP authentication username |
| `SMTP_PASSWORD` | Yes | -- | SMTP authentication password |
| `SMTP_FROM_EMAIL` | No | `noreply@kyomi.ai` | Sender email address |
| `SMTP_FROM_NAME` | No | `Kyomi` | Sender display name |

Kyomi considers SMTP configured when both `SMTP_HOST` and `SMTP_USER` are set.

### Testing email delivery

After configuring SMTP, create a new user account in Kyomi. If SMTP is working, the user will receive a verification email. Check `docker compose logs -f kyomi` for any SMTP errors if the email does not arrive.

---

## Slack Integration (Enterprise)

Slack integration allows your team to interact with Kyomi directly from Slack channels -- ask data questions, receive watch alerts, and share insights without leaving Slack.

**Requires:** Enterprise edition (`KYOMI_EDITION=enterprise`)

### Create a Slack App

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and click **Create New App**
2. Choose **From scratch**, give it a name (e.g., "Kyomi"), and select your workspace

### Configure OAuth Scopes

Under **OAuth & Permissions**, add these **Bot Token Scopes**:

- `chat:write` -- send messages
- `commands` -- handle slash commands
- `app_mentions:read` -- respond when @mentioned
- `channels:read` -- list channels
- `files:write` -- upload chart images
- `users:read` -- resolve user display names

### Set Redirect URL

Under **OAuth & Permissions** > **Redirect URLs**, add:

```
https://your-kyomi-domain/api/v1/slack/oauth/callback
```

Replace `your-kyomi-domain` with your actual Kyomi URL.

### Enable Events

Under **Event Subscriptions**:

1. Turn on **Enable Events**
2. Set the **Request URL** to:
   ```
   https://your-kyomi-domain/api/v1/slack/events
   ```
3. Under **Subscribe to bot events**, add:
   - `app_mention`
   - `message.channels`

### Add Slash Command

Under **Slash Commands**, create a new command:

- **Command:** `/kyomi`
- **Request URL:** `https://your-kyomi-domain/api/v1/slack/events`
- **Description:** Ask Kyomi a data question

### Configure Environment Variables

```env
SLACK_CLIENT_ID=your-slack-client-id
SLACK_CLIENT_SECRET=your-slack-client-secret
SLACK_SIGNING_SECRET=your-slack-signing-secret
```

Find these values on the **Basic Information** page of your Slack app.

All three variables must be set. Kyomi will log a warning at startup if `SLACK_CLIENT_ID` is set without `SLACK_SIGNING_SECRET`.

### Install the App

After configuring everything, go to **Settings > Kyomi > Integrations** in the Kyomi UI and click **Connect Slack**. This initiates the OAuth flow and installs the app to your Slack workspace.

---

## Google OAuth

Google OAuth enables two features:

1. **Google Sign-In** -- users can sign in to Kyomi with their Google account
2. **BigQuery datasource** -- users can connect their BigQuery projects using their Google credentials

### Create OAuth Credentials

1. Go to [console.cloud.google.com](https://console.cloud.google.com)
2. Create a project (or select an existing one)
3. Go to **APIs & Services > Credentials**
4. Click **Create Credentials > OAuth client ID**
5. Application type: **Web application**
6. Add an **Authorized redirect URI**:
   ```
   https://your-kyomi-domain/api/v1/auth/google/callback
   ```
7. Copy the **Client ID** and **Client Secret**

### Enable Required APIs

If users will connect BigQuery datasources, enable the **BigQuery API** in your Google Cloud project under **APIs & Services > Library**.

### Configuration

```env
GOOGLE_OAUTH_CLIENT_ID=your-client-id.apps.googleusercontent.com
GOOGLE_OAUTH_CLIENT_SECRET=GOCSPX-...
```

When these variables are set, the Kyomi login page will show a "Sign in with Google" button.

---

## Web Push Notifications

Push notifications deliver watch alerts directly to users' browsers, even when they are not actively using Kyomi.

### Generate VAPID Keys

VAPID (Voluntary Application Server Identification) keys are required for the Web Push protocol.

```bash
npx web-push generate-vapid-keys
```

This outputs a public key and a private key. The public key is served to browsers automatically; you only need to configure the private key.

### Configuration

```env
VAPID_PRIVATE_KEY=your-base64-encoded-private-key
VAPID_CONTACT=mailto:admin@yourdomain.com
```

| Variable | Description |
|----------|-------------|
| `VAPID_PRIVATE_KEY` | The private key from the VAPID key pair |
| `VAPID_CONTACT` | Contact information (mailto: URL or https: URL) for the push service to reach you if there are issues |

### How it works

When a user enables push notifications in the Kyomi UI, their browser subscribes to push events. When a watch detects an anomaly, Kyomi sends a push notification to all subscribed browsers for that user.

Without VAPID keys configured, the push notification option will not appear in the UI. Watch alerts will still be available in-app and (if SMTP is configured) via email.
