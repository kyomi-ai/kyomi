#!/bin/bash
set -e

# Export beta signups to CSV for email notifications
# Usage: ./scripts/export-beta-signups.sh [output_file]

# Find the git repository root
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "❌ Error: Not in a git repository"
    exit 1
fi

# Default output file
OUTPUT_FILE="${1:-/tmp/beta_signups_$(date +%Y%m%d_%H%M%S).csv}"

echo "📧 Exporting beta signups..."
echo "📁 Output file: $OUTPUT_FILE"

# Export all signups with marketing consent
podman exec kyomi-postgres psql -U kyomi -d kyomi << EOF
\copy (
    SELECT
        email,
        company_name,
        company_size,
        use_case,
        created_at,
        marketing_consent
    FROM beta_signups
    ORDER BY created_at DESC
) TO STDOUT WITH CSV HEADER
EOF > "$OUTPUT_FILE"

# Count results
LINE_COUNT=$(wc -l < "$OUTPUT_FILE")
SIGNUP_COUNT=$((LINE_COUNT - 1))  # Subtract header row

echo ""
echo "✅ Export complete!"
echo "📊 Total signups: $SIGNUP_COUNT"
echo "📄 File: $OUTPUT_FILE"
echo ""
echo "📧 To send launch notification emails:"
echo "   1. Import CSV into your email service (SendGrid, Mailchimp, etc.)"
echo "   2. Create a campaign with your launch announcement"
echo "   3. Send to all emails in the CSV"
echo ""
echo "💡 Or filter for only marketing consent:"
echo "   podman exec kyomi-postgres psql -U kyomi -d kyomi -c \"SELECT email FROM beta_signups WHERE marketing_consent = true;\""
