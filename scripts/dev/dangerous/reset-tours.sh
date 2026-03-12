#!/bin/bash

# Reset product tour status for a user
# Usage: ./reset-tours.sh [email]

set -e

# Get email from argument or use default
EMAIL=${1:-"jason@yellowgorilla.net"}

echo "Resetting tour status for: $EMAIL"

# Reset tours_completed in database
PGPASSWORD=password psql -h localhost -p 5432 -U kyomi -d kyomi -c "
UPDATE users
SET extra_metadata = json_build_object('tours_completed', '{}'::json)
WHERE email = '$EMAIL';

SELECT email, extra_metadata->'tours_completed' as tours
FROM users
WHERE email = '$EMAIL';
"

echo ""
echo "✅ Tour status reset! Refresh your browser to test."
echo ""
