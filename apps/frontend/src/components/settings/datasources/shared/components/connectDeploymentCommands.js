// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Shared deployment command generators for Kyomi Connect.
 * Used by both ConnectSetup (creation flow) and ConnectStatus (edit flow).
 */

export const DEPLOYMENT_TABS = [
  { id: 'linux', label: 'Linux / macOS' },
  { id: 'docker', label: 'Docker' },
  { id: 'kubernetes', label: 'Kubernetes' },
  { id: 'compose', label: 'Compose' },
];

const DEFAULT_PORTS = {
  postgres: '5432',
  redshift: '5432',
  mysql: '3306',
  clickhouse: '8123',
  sqlserver: '1433',
  synapse: '1433',
};

export function getDefaultPort(datasourceType) {
  return DEFAULT_PORTS[datasourceType] || '5432';
}

function getDockerCommand(token, datasourceType) {
  const port = getDefaultPort(datasourceType);
  return `# Use "host.docker.internal" for DB_HOST if your database is on localhost
docker run -d \\
  --restart=always \\
  --name kyomi-connect \\
  -e KYOMI_TOKEN="${token}" \\
  -e DB_HOST="your-database-host" \\
  -e DB_PORT="${port}" \\
  -e DB_NAME="your-database" \\
  -e DB_USER="your-username" \\
  -e DB_PASSWORD="your-password" \\
  ghcr.io/kyomi-ai/kyomi-connect:latest`;
}

function getLinuxCommands(token) {
  if (token && !token.startsWith('<')) {
    return `# Install Kyomi Connect and run setup
curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token "${token}"`;
  }
  return `# Install Kyomi Connect and run interactive setup
curl -fsSL https://connect.kyomi.ai/install.sh | sh`;
}

function getKubernetesCommands(token, datasourceType) {
  const port = getDefaultPort(datasourceType);
  return `# Create the token secret
kubectl create secret generic kyomi-connect-token \\
  --from-literal=token="${token}"

# Create the database password secret
kubectl create secret generic kyomi-connect-db \\
  --from-literal=password="your-password"

# Install with Helm (OCI registry)
helm install kyomi-connect \\
  oci://ghcr.io/kyomi-ai/charts/kyomi-connect \\
  --set existingSecret.name=kyomi-connect-token \\
  --set target.host="your-database-host" \\
  --set target.port=${port} \\
  --set target.database="your-database" \\
  --set target.user="your-username" \\
  --set target.passwordSecretName=kyomi-connect-db`;
}

function getComposeSnippet(token, datasourceType) {
  const port = getDefaultPort(datasourceType);
  return `# Use "host.docker.internal" for DB_HOST if your database is on localhost
services:
  kyomi-connect:
    image: ghcr.io/kyomi-ai/kyomi-connect:latest
    restart: always
    environment:
      KYOMI_TOKEN: "${token}"
      DB_HOST: "your-database-host"
      DB_PORT: "${port}"
      DB_NAME: "your-database"
      DB_USER: "your-username"
      DB_PASSWORD: "your-password"`;
}

export function getTabContent(tabId, token, datasourceType) {
  switch (tabId) {
    case 'docker':
      return getDockerCommand(token, datasourceType);
    case 'linux':
      return getLinuxCommands(token);
    case 'kubernetes':
      return getKubernetesCommands(token, datasourceType);
    case 'compose':
      return getComposeSnippet(token, datasourceType);
    default:
      return '';
  }
}
