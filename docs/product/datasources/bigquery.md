# Connecting to Google BigQuery

Kyomi supports three authentication methods for BigQuery. Choose the one that best fits your organization's security requirements.

## Which authentication method should I use?

| Method | Best For | Setup Complexity |
|--------|----------|------------------|
| **Kyomi OAuth** | Individuals, small teams | Easy - just click "Connect BigQuery" |
| **Service Account** | Shared team access, automation | Medium - create key in GCP Console |
| **Enterprise OAuth** | Organizations with Google Workspace | Advanced - configure OAuth consent screen |

---

## Method 1: Kyomi OAuth

The simplest way to connect - sign in with your Google account and authorize Kyomi to access BigQuery.

### Setup Steps

1. In the datasource modal, select **BigQuery** as the datasource type
2. Choose **Kyomi OAuth** as the authentication method (this is the default)
3. Click **Connect BigQuery**
4. Sign in with your Google account and grant the requested permissions
5. Select your **Billing Project** (the GCP project that will be charged for queries)
6. Optionally select a **Default Project** for queries
7. Click **Next**, then **Create**

::: info Note
Your Google account must have the necessary BigQuery permissions (e.g., `bigquery.jobs.create`, `bigquery.tables.getData`) in the projects you want to query.
:::

---

## Method 2: Service Account

Use a service account for shared, credential-based access. All workspace users will use the same service account.

### Prerequisites

- Access to the [Google Cloud Console](https://console.cloud.google.com)
- Permission to create service accounts in your GCP project

### Step 1: Create a Service Account

1. Go to [IAM & Admin → Service Accounts](https://console.cloud.google.com/iam-admin/serviceaccounts)
2. Click **Create Service Account**
3. Enter a name (e.g., "kyomi-bigquery-access")
4. Click **Create and Continue**
5. Grant the following roles:
   - `BigQuery Data Viewer` - to read table data
   - `BigQuery Job User` - to run queries
   - `BigQuery Metadata Viewer` - to browse schemas (optional but recommended)
6. Click **Done**

### Step 2: Create a Key

1. Click on the service account you just created
2. Go to the **Keys** tab
3. Click **Add Key → Create new key**
4. Select **JSON** format
5. Click **Create** - the key file will download automatically

::: warning Security
Keep this JSON file secure. Anyone with this file has access to your BigQuery data. The key is encrypted at rest when stored in Kyomi.
:::

### Step 3: Configure in Kyomi

1. In the datasource modal, select **BigQuery** as the datasource type
2. Choose **Service Account** as the authentication method
3. Upload the JSON key file or paste its contents
4. Click **Validate & Discover Projects** to verify credentials and load available projects
5. Select your **Billing Project** and **Default Project**
6. Click **Save**

---

## Method 3: Enterprise OAuth

Configure your own OAuth client for branded consent screens and per-user audit trails. This method requires Google Cloud Console access and is typically set up by IT administrators.

### Prerequisites

- Google Cloud project with BigQuery API enabled
- Permission to configure OAuth consent screens and create OAuth clients
- Domain ownership for production OAuth apps (or use internal-only apps for Google Workspace)

### Step 1: Configure OAuth Consent Screen

1. Go to [APIs & Services → OAuth consent screen](https://console.cloud.google.com/apis/credentials/consent)
2. Select **Internal** (for Google Workspace organizations) or **External**
3. Fill in your app information:
   - App name: Your company name or "Kyomi Analytics"
   - User support email: Your IT support email
   - Authorized domains: Add your company domain
4. Add scopes:
   - `https://www.googleapis.com/auth/bigquery`
   - `https://www.googleapis.com/auth/cloud-platform.read-only` (for project listing)
5. Complete the consent screen setup

### Step 2: Create OAuth Client

1. Go to [APIs & Services → Credentials](https://console.cloud.google.com/apis/credentials)
2. Click **Create Credentials → OAuth client ID**
3. Select **Web application**
4. Add authorized redirect URI:
   - `https://app.kyomi.ai/auth/oauth/bigquery-enterprise/callback`
5. Click **Create** and copy the **Client ID** and **Client Secret**

### Step 3: Configure in Kyomi

1. In the datasource modal, select **BigQuery** as the datasource type
2. Choose **Enterprise OAuth** as the authentication method
3. Enter the **Client ID** and **Client Secret**
4. Save the connection settings
5. Each user will need to click **Connect with Google** to authenticate

---

## Troubleshooting

### "Access Denied" or "Permission Denied" errors

- Verify your account has the required BigQuery roles in the GCP project
- For service accounts, ensure the roles are granted at the project level
- Check that the BigQuery API is enabled in your GCP project

### OAuth consent screen shows "unverified app" warning

- For internal apps: Use the "Internal" user type in OAuth consent screen settings
- For external apps: Submit your app for Google verification, or users can click "Advanced" → "Go to [app]"

### Can't see all projects in the dropdown

- Your account needs `resourcemanager.projects.get` permission on the projects
- Try typing the project ID directly if you know it

---

## Additional Resources

- [BigQuery Quickstart Guide](https://cloud.google.com/bigquery/docs/quickstarts)
- [BigQuery Access Control](https://cloud.google.com/bigquery/docs/access-control)
- [Understanding Service Accounts](https://cloud.google.com/iam/docs/service-accounts)

---

[← Back to Docs](/docs/)
