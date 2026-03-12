# Role Naming Convention

## Overview
The application uses different role naming formats in different layers of the system. This document clarifies the correct usage.

## Role Formats

### API Layer (Frontend → Backend)
The API accepts and returns these role values:
- `"admin"` - Workspace administrator
- `"user"` - Regular workspace user

**Used in:**
- `CreateInvitationRequest.role`
- `UpdateMemberRoleRequest.role`
- Invitation acceptance flow

### Database Layer
The database stores these role values in the `workspace_users` table:
- `"workspace_admin"` - Full workspace management rights
- `"workspace_user"` - Standard workspace user
- `"workspace_viewer"` - Read-only access (if implemented)

**Defined in:**
- `apps/backend/src/api/workspaces/workspace_models.py` - `WorkspaceRole` enum
- `apps/backend/src/api/database/models.py` - `WorkspaceUser.role` column

**Used in:**
- All database queries checking user permissions
- `MemberResponse.role` (returns database format to frontend)

### Invitations Table
The `workspace_invitations` table stores roles in API format:
- `"admin"` - Will become workspace_admin when accepted
- `"user"` - Will become workspace_user when accepted

## Conversion Points

### 1. Creating Invitations
```python
# API receives: "admin" or "user"
# Stored in invitations table: "admin" or "user" (no conversion)
```

### 2. Accepting Invitations
```python
# Read from invitations: "admin" or "user"
# Convert and store in workspace_users:
role="workspace_admin" if invitation.role == "admin" else "workspace_user"
```

### 3. Updating Member Roles
```python
# API receives: "admin" or "user"
# Validate: request.role in ["admin", "user"]
# Convert and store in workspace_users:
member.role = "workspace_admin" if request.role == "admin" else "workspace_user"
```

### 4. Ownership Transfer
```python
# On accept, ensure new owner is admin:
if new_owner_membership.role != "workspace_admin":
    new_owner_membership.role = "workspace_admin"
```

## Frontend Role Display

The frontend receives database format (`"workspace_admin"` or `"workspace_user"`) but displays and sends API format (`"admin"` or `"user"`):

```jsx
// Display conversion
<Select value={member.role === 'workspace_admin' ? 'admin' : 'user'}>
  <SelectItem value="user">User</SelectItem>
  <SelectItem value="admin">Admin</SelectItem>
</Select>

// Sends "admin" or "user" to API
onValueChange={(value) => handleUpdateMemberRole(member.user_id, value)}
```

## Permission Checks

Always check against database format when verifying permissions:

```python
# ✅ Correct
if workspace_user.role != "workspace_admin":
    raise HTTPException(status_code=403, detail="Admin only")

# ❌ Wrong
if workspace_user.role != "admin":  # Will never match!
    raise HTTPException(status_code=403, detail="Admin only")
```

## Migration from Invalid Data

If you find users with `role='user'` or `role='admin'` in the `workspace_users` table (instead of the proper `workspace_` prefixed values), fix them:

```sql
-- Fix invalid user roles
UPDATE workspace_users
SET role = 'workspace_user'
WHERE role = 'user';

-- Fix invalid admin roles
UPDATE workspace_users
SET role = 'workspace_admin'
WHERE role = 'admin';

-- Verify no invalid roles remain
SELECT user_id, role
FROM workspace_users
WHERE role NOT IN ('workspace_admin', 'workspace_user', 'workspace_viewer');
```

## Summary

| Layer | Admin Role | User Role |
|-------|-----------|-----------|
| **API (Input/Output)** | `"admin"` | `"user"` |
| **Database (workspace_users)** | `"workspace_admin"` | `"workspace_user"` |
| **Invitations (workspace_invitations)** | `"admin"` | `"user"` |
| **Frontend Display** | `"admin"` | `"user"` |
| **Frontend Receives** | `"workspace_admin"` | `"workspace_user"` |

**Key Rule**: API uses short names, database uses `workspace_` prefix. Convert at the boundary.
