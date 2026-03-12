# Adding New Datasource Types

This guide documents the architecture and process for adding new datasource types to Kyomi (e.g., PostgreSQL, ClickHouse, Snowflake, etc.).

## Architecture Overview

Kyomi uses a **registry-based provider pattern** where each datasource type self-registers its metadata. The agent tools and routers are **generic** - they use the registry for all routing decisions, eliminating switch statements.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                           Agent Tools (Generic)                          │
│  search_catalog() - returns tables from ALL datasources with type labels │
│  query_datasource() - routes to correct provider via registry lookup    │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                    DatasourceTypeRegistry (Central)                      │
│  Self-registration on import - each provider declares its metadata      │
│  Lazy loading - providers imported only when needed                     │
│  API endpoint: GET /api/v1/datasources/types                            │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
            ┌───────────────────────┼───────────────────────┐
            ▼                       ▼                       ▼
┌───────────────────┐   ┌───────────────────┐   ┌───────────────────┐
│  BigQueryProvider │   │  PostgresProvider │   │  ClickHouseProvider│
│  + Metadata       │   │  + Metadata       │   │  + Metadata        │
│  (OAuth-based)    │   │  (SSH tunnel opt) │   │  (Direct conn)     │
└───────────────────┘   └───────────────────┘   └───────────────────┘
```

### Key Principles

1. **Self-Registration**: Each provider package registers its metadata in `__init__.py`
2. **No Switch Statements**: All routing uses `DatasourceTypeRegistry.get(type_id)`
3. **Lazy Loading**: Provider classes imported only when needed
4. **Single Source of Truth**: All metadata lives in one place per provider
5. **API-Driven UI**: Frontend queries `/api/v1/datasources/types` for dynamic rendering

### Before/After Registry - Switch Statement Elimination

The registry architecture eliminates switch statements throughout the codebase. Here's how:

**BEFORE Registry (Switch Statement Hell):**

```python
# Example 1: Catalog tree building (routers/datasources.py)
if datasource_type == "bigquery":
    level1_type = "project"
    level2_type = "dataset"
    skip_empty = False
    skip_single = False
elif datasource_type == "postgres":
    level1_type = "database"
    level2_type = "schema"
    skip_empty = False
    skip_single = True
elif datasource_type == "mysql":
    level1_type = None
    level2_type = "database"
    skip_empty = True
    skip_single = False
elif datasource_type == "clickhouse":
    level1_type = None
    level2_type = "database"
    skip_empty = True
    skip_single = False
# ... 10+ more cases, duplicated in multiple files!

# Example 2: Provider class lookup (services/datasource_provider_service.py)
if datasource_type == "bigquery":
    from api.datasources.bigquery import BigQueryProvider
    return BigQueryProvider
elif datasource_type == "postgres":
    from api.datasources.postgres import PostgresProvider
    return PostgresProvider
elif datasource_type == "mysql":
    from api.datasources.mysql import MySQLProvider
    return MySQLProvider
# ... more cases, had to update every time we added a datasource

# Example 3: Catalog config extraction (routers/datasources.py)
if datasource_type == "bigquery":
    catalog_config = {
        "catalog_projects": conn_config.get("catalog_projects", []),
        "include_public_datasets": conn_config.get("include_public_datasets", False)
    }
elif datasource_type == "postgres":
    catalog_config = {
        "catalog_schemas": conn_config.get("catalog_schemas", [])
    }
elif datasource_type == "mysql":
    catalog_config = {
        "catalog_databases": conn_config.get("catalog_databases", [])
    }
# ... more duplication
```

**AFTER Registry (Clean, Extensible):**

```python
# Example 1: Catalog tree building - ONE LINE!
meta = DatasourceTypeRegistry.get(datasource_type)
level1_type = meta.tree_level1_type
level2_type = meta.tree_level2_type
skip_empty = meta.skip_empty_project_wrapper
skip_single = meta.skip_single_project_wrapper

# Example 2: Provider class lookup - ONE LINE!
ProviderClass = DatasourceTypeRegistry.get_provider_class(datasource_type)
return ProviderClass(connection_config, credentials)

# Example 3: Catalog config extraction - ONE LINE!
meta = DatasourceTypeRegistry.get(datasource_type)
catalog_config = {
    key: conn_config.get(key, [] if key != "include_public_datasets" else False)
    for key in meta.catalog_config_keys
}

# Example 4: Discovery method routing - ONE LINE!
meta = DatasourceTypeRegistry.get(datasource_type)
discovery_method = getattr(provider, meta.discovery_method)
discovery = discovery_method()
```

**Key Benefits:**

1. **Zero Maintenance**: Add new datasource, no router changes needed
2. **No Duplication**: Metadata declared once in provider's `__init__.py`
3. **Type Safety**: Metadata validation at registration time
4. **Self-Documenting**: All datasource capabilities in one place
5. **API Exposure**: Frontend can query `/api/v1/datasources/types` for dynamic UI

**Adding a New Datasource:**

- **Old Way**: Update 8+ switch statements across 4+ files
- **New Way**: Register metadata in provider's `__init__.py`, done!

## What's New in This Guide

This guide has been updated based on the DRY architecture refactoring. Key additions:

### DRY Architecture (Reduces Code Duplication)

1. **BaseSQLCatalogIndexer** - Base class for all SQL-based catalog indexers
   - Implement just 5 abstract methods instead of 700+ lines
   - See: [Catalog Indexer](#3-catalog-indexer)

2. **SSHTunnelMixin** - Reusable SSH tunnel support
   - Just add the mixin to your provider class
   - See: [SSH Tunnel Support](#ssh-tunnel-support-via-mixin)

3. **SharedCredentialsMixin** - Reusable shared credentials support
   - Just add the mixin to your provider class
   - See: [Shared Credentials](#shared-credentials-via-mixin)

4. **genericProxyDataSource** - Unified ChartML plugin
   - No separate plugin file needed per datasource
   - Just add your type to PROXY_DATASOURCES array
   - See: [ChartML Data Source Plugin](#10-chartml-data-source-plugin-critical-for-dashboards)

### Core Features

5. **Query Pagination** - Production-ready pagination with total counts
   - LIMIT/OFFSET support
   - Intelligent existing-LIMIT handling
   - See: [Query Pagination Support](#query-pagination-support-recommended)

6. **Error Location Parsing** - Complete PostgreSQL reference implementation
   - Character position to line number conversion
   - See: [Parsing Error Locations](#parsing-error-locations-provider-specific)

7. **Type Mapping** - Comprehensive PostgreSQL example with 40+ types
   - JSON, UUID, array handling
   - Preserve date/time granularity
   - See: [Date/Time Type Mapping](#datetime-type-mapping-critical)

### Reference Implementation

The **PostgreSQL provider** is now documented as the gold standard reference with all features implemented. See: [PostgreSQL: Reference Implementation](#postgresql-reference-implementation)

### Quick Start - New Provider Checklist

Adding a new SQL-based datasource requires:
1. **Provider** (~200 lines): Extend `BaseDatasourceProvider`, use mixins for SSH/shared credentials
2. **Indexer** (~50 lines): Extend `BaseSQLCatalogIndexer`, implement 5 abstract methods
3. **Registry Registration** (~30 lines): Self-register metadata in package `__init__.py`
4. **ChartML Plugin** (0 lines): Add type to `PROXY_DATASOURCES` in `genericProxyDataSource.js`
5. **Frontend Form** (~80 lines): Add connection/credentials forms in `DatasourceSettings.jsx`
6. **Validation Schemas** (~40 lines): Add Pydantic models in `routers/datasources.py`

**No switch statements to update!** The registry-based architecture eliminates manual routing code.

## Files to Create/Modify

### 1. Backend Provider Package

**Create:** `apps/backend/src/api/datasources/<type>/__init__.py`

```python
"""
<Type> Datasource Provider

<Brief description of the datasource and any special features>
"""

from .provider import <Type>Provider
from .indexer import <Type>CatalogIndexer

__all__ = ["<Type>Provider", "<Type>CatalogIndexer"]
```

### 2. Provider Implementation

**Create:** `apps/backend/src/api/datasources/<type>/provider.py`

The provider must implement the `BaseDatasourceProvider` interface. Use the provided mixins for SSH tunnel and shared credentials support:

```python
from ..base import BaseDatasourceProvider, QueryResult
from ..mixins import SSHTunnelMixin, SharedCredentialsMixin  # Use mixins!
from typing import Dict, Any, Optional
import logging

logger = logging.getLogger(__name__)

class <Type>Provider(SSHTunnelMixin, SharedCredentialsMixin, BaseDatasourceProvider):
    """
    <Type> provider implementation.

    Uses:
    - SSHTunnelMixin: Provides _create_ssh_tunnel() and _close_ssh_tunnel()
    - SharedCredentialsMixin: Provides _resolve_credentials()

    Connection config (workspace-level, stored in DatasourceConfig):
        - host: Database host
        - port: Database port (default: <default_port>)
        - database: Database name
        - ssh_enabled, ssh_host, ssh_port, ssh_username, ssh_private_key
        - shared_credentials, shared_username, shared_password

    Credentials (user-level, stored in UserDatasourceCredential):
        - username: Database username
        - password: Database password
    """

    DATASOURCE_TYPE = "<type>"  # Must match database enum value

    def __init__(self, connection_config: Dict[str, Any], credentials: Dict[str, Any] = None):
        """
        Initialize provider with connection config and user credentials.

        Args:
            connection_config: Workspace-level config (host, port, etc.)
            credentials: User-level credentials (username, password)
        """
        self.connection_config = connection_config
        self.credentials = credentials or {}
        self._connection = None
        self._tunnel = None  # Required by SSHTunnelMixin

    def _get_connection(self):
        """Get or create database connection with SSH tunnel and shared credentials support."""
        if self._connection is not None:
            return self._connection

        # SSH tunnel support (from SSHTunnelMixin)
        if self.connection_config.get("ssh_enabled"):
            connect_host, connect_port = self._create_ssh_tunnel(
                target_host=self.connection_config.get("host", "localhost"),
                target_port=self.connection_config.get("port", <default_port>)
            )
        else:
            connect_host = self.connection_config.get("host", "localhost")
            connect_port = self.connection_config.get("port", <default_port>)

        # Shared credentials support (from SharedCredentialsMixin)
        username, password = self._resolve_credentials()

        # Create connection using connect_host, connect_port, username, password
        # self._connection = ...

        return self._connection

    def execute_query(self, sql: str, limit: Optional[int] = None) -> QueryResult:
        """Execute SQL query and return results."""
        # Implementation here
        pass

    def test_connection(self) -> bool:
        """Test that the connection works. Returns True if successful, False otherwise."""
        pass

    def close(self):
        """Clean up connection and SSH tunnel."""
        if self._connection:
            try:
                self._connection.close()
            except Exception as e:
                logger.warning(f"Error closing connection: {e}")
            self._connection = None

        self._close_ssh_tunnel()  # From SSHTunnelMixin - safe to call even if no tunnel
```

**Key Methods:**
- `execute_query(sql, limit)` - Execute query, return `QueryResult`
- `dry_run(sql)` - Validate query syntax without executing, return `DryRunResult`
- `get_table_info(table_name)` - Get detailed table schema for agent tools
- `test_connection()` - Validate connection works
- `close()` - Clean up resources

### get_table_info() Method (Required for Agent)

The `get_table_info()` method provides detailed schema information for agent tools. This is used by the `get_table_info` tool to help the AI understand table structure before writing queries.

```python
def get_table_info(self, table_name: str) -> Dict[str, Any]:
    """
    Get detailed schema information for a table.

    Args:
        table_name: Fully qualified table name
            - PostgreSQL: schema.table
            - ClickHouse: database.table
            - BigQuery: project.dataset.table

    Returns:
        Dict with standardized schema info:
        {
            "table": str,          # Full table name
            "description": str,    # Table description (if available)
            "rows": int|None,      # Row count (if available)
            "cols": [              # Column list
                {"name": str, "type": str, "description": str},
                ...
            ]
        }

        Or on error:
        {"error": str}
    """
    # Parse table name based on your datasource format
    parts = table_name.split('.')
    if len(parts) != 2:  # Adjust for your format
        return {"error": f"Invalid table name format: {table_name}"}

    # Query system catalog for column info
    # Example for PostgreSQL:
    columns_sql = """
        SELECT column_name, data_type, col_description(...)
        FROM information_schema.columns
        WHERE table_schema = %s AND table_name = %s
    """
    # Execute and format results
    return {
        "table": table_name,
        "description": table_description,
        "rows": row_count,
        "cols": columns
    }
```

**Provider Examples:**

| Datasource | System Catalog | Table Name Format |
|------------|---------------|-------------------|
| PostgreSQL | `information_schema.columns`, `pg_catalog.pg_description` | `schema.table` |
| ClickHouse | `system.columns`, `system.tables` | `database.table` |
| BigQuery | BigQuery Table API | `project.dataset.table` |

**QueryResult Format:**
```python
{
    "status": "success",
    "columns": ["col1", "col2"],
    "data": {"col1": [val1, val2], "col2": [val3, val4]},  # Columnar format
    "row_count": 2,
    "truncated": False
}
```

**DryRunResult Format:**

The `dry_run()` method validates query syntax without executing. It returns a `DryRunResult` with:
- `valid`: Boolean - drives UI color (green/red) and icon (✓/✗)
- `message`: String - provider-formatted message to display
- `line`: Optional int - error line number (1-indexed) for editor markers
- `column`: Optional int - error column number (requires line)

```python
from ..base import DryRunResult

def dry_run(self, sql: str) -> DryRunResult:
    """Validate query syntax using EXPLAIN."""
    try:
        conn = self._get_connection()
        with conn.cursor() as cursor:
            cursor.execute(f"EXPLAIN {sql}")
        return DryRunResult(valid=True, message="Query valid")
    except Exception as e:
        # Parse error for line/column location (provider-specific)
        line = self._parse_error_location(e, sql)
        return DryRunResult(
            valid=False,
            message=str(e),
            line=line,
            # column=column,  # If your database provides it
        )
```

**Parsing Error Locations (Provider-Specific):**

Each database returns errors in different formats. The provider is responsible for parsing its own error format:

| Database | Error Format | Example |
|----------|-------------|---------|
| BigQuery | `at [line:column]` | `Syntax error: Expected ')' at [4:3]` |
| PostgreSQL | `diag.statement_position` | Character position, convert to line |
| ClickHouse | TBD | Document when implemented |
| Snowflake | TBD | Document when implemented |

**PostgreSQL Reference Implementation (psycopg2):**

This is the complete, production-tested implementation from `apps/backend/src/api/datasources/postgres/provider.py`:

```python
def _parse_postgres_error_location(self, error: Exception, sql: str) -> Optional[int]:
    """
    Extract line number from PostgreSQL error.

    psycopg2 errors have diag.statement_position which is character position.
    We convert to line number by counting newlines in SQL up to that position.

    Args:
        error: psycopg2 exception
        sql: Original SQL query

    Returns:
        Line number (1-indexed) or None if not available
    """
    try:
        # psycopg2 errors have diagnostic info
        if hasattr(error, 'diag') and error.diag:
            position = error.diag.statement_position
            if position:
                # Convert character position to line number
                char_pos = int(position) - 1  # Convert to 0-indexed
                if 0 <= char_pos < len(sql):
                    return sql[:char_pos].count('\n') + 1
    except Exception:
        pass
    return None
```

**Usage in dry_run():**

```python
def dry_run(self, sql: str) -> DryRunResult:
    """Validate query without executing using PostgreSQL EXPLAIN."""
    try:
        conn = self._get_connection()
        explain_sql = f"EXPLAIN {sql}"
        with conn.cursor() as cursor:
            cursor.execute(explain_sql)
        conn.rollback()  # Don't commit EXPLAIN
        return DryRunResult(
            valid=True,
            message="Query valid",
        )
    except Exception as e:
        # Rollback on error
        try:
            if self._connection:
                self._connection.rollback()
        except Exception:
            pass

        # Extract line number from PostgreSQL error
        line = self._parse_postgres_error_location(e, sql)

        return DryRunResult(
            valid=False,
            message=str(e),
            line=line,
            # PostgreSQL doesn't provide column, only character position
        )
```

Example for BigQuery:
```python
def _parse_error_location(self, error_message: str) -> tuple[Optional[int], Optional[int]]:
    """Parse BigQuery error for line/column."""
    import re
    match = re.search(r'at\s+\[(\d+):(\d+)\]', error_message)
    if match:
        return int(match.group(1)), int(match.group(2))
    return None, None
```

The frontend uses `line` and `column` to set error markers in the Monaco editor, providing visual feedback for syntax errors.

### Date/Time Type Mapping (CRITICAL)

**IMPORTANT: Preserve granular date/time types for proper frontend display.**

Databases distinguish between DATE, TIME, TIMESTAMP, and TIMESTAMPTZ. Your provider MUST preserve these distinctions instead of collapsing them to a generic "datetime" type.

**Required Type Mappings:**

| Database Type | Simple Type | Display Format | Example |
|---------------|-------------|----------------|---------|
| DATE | `"date"` | YYYY-MM-DD | `2024-01-15` |
| TIME | `"time"` | HH:MM:SS | `10:30:45` |
| TIMESTAMP (no TZ) | `"timestamp"` | YYYY-MM-DD HH:MM:SS | `2024-01-15 10:30:45` |
| TIMESTAMP WITH TZ | `"timestamptz"` | YYYY-MM-DD HH:MM:SS+00:00 | `2024-01-15 10:30:45+00:00` |

**Provider Examples:**

PostgreSQL (OID-based mapping):
```python
PG_TYPE_OID_MAP = {
    1082: "date",        # date
    1083: "time",        # time
    1114: "timestamp",   # timestamp without timezone
    1184: "timestamptz", # timestamp with timezone
}
```

BigQuery (string-based mapping):
```python
def _map_bigquery_type(self, bq_type: str) -> str:
    type_upper = bq_type.upper()
    if type_upper == "DATE":
        return "date"
    elif type_upper == "DATETIME":
        return "timestamp"  # DATETIME is datetime without timezone
    elif type_upper == "TIMESTAMP":
        return "timestamptz"  # TIMESTAMP has timezone
    elif type_upper == "TIME":
        return "time"
```

ClickHouse (string-based mapping):
```python
def _map_clickhouse_type(self, ch_type: str) -> str:
    type_lower = ch_type.lower()
    # Check specific types first (datetime64 before datetime, date32 before date)
    if type_lower.startswith("datetime64") or type_lower.startswith("datetime"):
        return "timestamp"
    elif type_lower.startswith("date32") or type_lower.startswith("date"):
        return "date"
```

**Why This Matters:**

1. **User Experience**: Users see clean dates (`2024-01-15`) instead of ugly timestamps (`2024-01-15T00:00:00.000Z`)
2. **Semantic Correctness**: Frontend displays values based on their actual database type
3. **Chart Compatibility**: Charts handle time axes correctly
4. **Type Preservation**: JavaScript Date objects maintain database semantics

**Data Flow:**

```
Database (native DATE/TIMESTAMP types)
  ↓ Provider type mapping
Backend API (granular types: "date", "timestamp", "timestamptz")
  ↓ JSON serialization (ISO strings)
Frontend (parseDateColumns converts to Date objects)
  ↓ DuckDB storage (native TIMESTAMP types)
  ↓ Query results (Date objects)
Table Display (formats based on original type)
  → date: "2024-01-15"
  → timestamp: "2024-01-15 10:30:45"
  → timestamptz: "2024-01-15 10:30:45+00:00"
```

**Common Mistake:**

❌ **DON'T** collapse all date types to "datetime":
```python
# WRONG - loses type information
1082: "datetime",  # date
1114: "datetime",  # timestamp
1184: "datetime",  # timestamptz
```

✅ **DO** preserve granular types:
```python
# CORRECT - preserves type information
1082: "date",        # date
1114: "timestamp",   # timestamp
1184: "timestamptz", # timestamptz
```

### Advanced Type Mapping - Complete PostgreSQL Example

The PostgreSQL provider includes comprehensive type mapping beyond just date/time types. This serves as a reference for implementing complete type support:

**Reference:** `apps/backend/src/api/datasources/postgres/provider.py` (lines 23-64)

```python
# PostgreSQL type OIDs to simple types mapping
# See: https://www.postgresql.org/docs/current/datatype-oid.html
PG_TYPE_OID_MAP = {
    # Boolean
    16: "boolean",      # bool

    # Numbers
    20: "number",       # int8 (bigint)
    21: "number",       # int2 (smallint)
    23: "number",       # int4 (integer)
    26: "number",       # oid
    700: "number",      # float4 (real)
    701: "number",      # float8 (double precision)
    1700: "number",     # numeric

    # Strings
    18: "string",       # char
    19: "string",       # name
    25: "string",       # text
    1042: "string",     # bpchar (char(n))
    1043: "string",     # varchar

    # Date/Time - Granular types preserved for frontend display
    1082: "date",        # date (YYYY-MM-DD)
    1083: "time",        # time (HH:MM:SS)
    1114: "timestamp",   # timestamp without timezone
    1184: "timestamptz", # timestamp with timezone
    1186: "time",        # interval (display as time)

    # JSON
    114: "string",      # json
    3802: "string",     # jsonb

    # UUID
    2950: "string",     # uuid

    # Arrays (treat as string for display)
    1009: "string",     # text[]
    1015: "string",     # varchar[]
    1016: "string",     # int8[]
    1007: "string",     # int4[]
}
```

**Best Practices:**
- Add inline comments explaining database-specific type codes
- Group related types (numbers, strings, dates, etc.)
- Handle special types (JSON, UUID, arrays) appropriately
- Default to "unknown" for unmapped types rather than failing

### Value Conversion Pattern (CRITICAL for REST API Providers)

**Problem:** Database drivers (psycopg2, mysql-connector, clickhouse-connect) return **native Python types** (int, float, bool, datetime). REST APIs (BigQuery, some Snowflake endpoints) return **everything as strings**.

**The Data Flow:**

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           DATABASE DRIVER PATH                               │
│  (PostgreSQL, MySQL, ClickHouse, SQL Server, DuckDB)                        │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Database → Driver returns native Python types → _convert_value() →         │
│  JSON serialization → Frontend receives proper types                        │
│                                                                              │
│  Example: cursor returns (1, 3.14, True, datetime(2024,1,15))               │
│           _convert_value serializes datetime → ISO string                    │
│           JSON: [1, 3.14, true, "2024-01-15T00:00:00"]                      │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                           REST API PATH                                      │
│  (BigQuery, potentially Snowflake/Databricks REST endpoints)                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  REST API returns strings → _convert_rest_value(value, schema_type) →       │
│  Native Python types → _convert_value() → JSON → Frontend                   │
│                                                                              │
│  Example: API returns {"v": "1"}, {"v": "3.14"}, {"v": "true"}              │
│           _convert_rest_value parses using schema: int, float, bool         │
│           JSON: [1, 3.14, true, "2024-01-15"]                               │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**What to Convert vs Keep as String:**

| Type Category | REST API Returns | Convert To | Why |
|---------------|------------------|------------|-----|
| INTEGER/INT64 | `"1"` | `int(value)` | Math operations, sorting |
| FLOAT/FLOAT64/NUMERIC | `"3.14"` | `float(value)` | Math operations |
| BOOLEAN/BOOL | `"true"` | `value == "true"` | Boolean logic |
| DATE | `"2024-01-15"` | Keep as string | Already ISO format, frontend parses |
| DATETIME | `"2024-01-15 10:30:45"` | Keep as string | Already ISO format, frontend parses |
| TIMESTAMP | `"1705312245.123"` | Keep as string OR convert | BigQuery uses epoch seconds |
| TIME | `"10:30:45"` | Keep as string | No native JSON time type |
| STRING/BYTES | `"hello"` | Keep as string | Already correct type |

**Key Insight:** Dates/timestamps can stay as ISO strings because:
1. JSON has no native date type (serializes to string anyway)
2. Frontend `parseDateColumns()` converts ISO strings → JavaScript Date objects
3. Column metadata (`type: "date"`, `type: "timestamp"`) tells frontend how to parse

**Reference Implementation - Backend:**

```python
def _convert_rest_value(self, value: str, field_type: str) -> Any:
    """
    Convert REST API string value to native Python type.

    Called BEFORE _convert_value() for REST API responses.
    """
    if value is None:
        return None

    type_upper = field_type.upper()

    # Numbers - MUST convert for math operations
    if type_upper in ("INTEGER", "INT64"):
        return int(value)
    if type_upper in ("FLOAT", "FLOAT64", "NUMERIC", "BIGNUMERIC"):
        return float(value)

    # Booleans - MUST convert for boolean logic
    if type_upper in ("BOOLEAN", "BOOL"):
        return value.lower() == "true"

    # Dates/timestamps - keep as ISO string (frontend handles)
    # STRING, BYTES, DATE, DATETIME, TIMESTAMP, TIME, GEOGRAPHY, etc.
    return value
```

**Reference Implementation - Frontend (BigQueryDirectService.js):**

```javascript
_convertValue(value, columnType) {
    switch (columnType) {
        case 'INTEGER':
        case 'INT64':
            return parseInt(value, 10);
        case 'FLOAT':
        case 'FLOAT64':
        case 'NUMERIC':
            return parseFloat(value);
        case 'BOOLEAN':
        case 'BOOL':
            return value === 'true' || value === true;
        case 'TIMESTAMP':
            // BigQuery: epoch seconds → Date
            return new Date(parseFloat(value) * 1000);
        case 'DATE':
        case 'DATETIME':
            return new Date(value);
        default:
            return value;
    }
}
```

**Integration with Base Class:**

The base `BaseDatasourceProvider` provides `_convert_value()` for JSON serialization:

```python
# In execute_query():

# For driver-based providers (PostgreSQL, MySQL, etc.):
rows = self._process_cursor_results(cursor.fetchall(), columns)
# _process_cursor_results calls _convert_value on each value

# For REST API providers (BigQuery):
for row in api_response["rows"]:
    converted_row = []
    for i, cell in enumerate(row["f"]):
        value = cell["v"]
        field_type = schema_fields[i]["type"]
        # First: parse string to native type
        native_value = self._convert_rest_value(value, field_type)
        # Then: _convert_value handles JSON serialization (optional, mostly passthrough)
        converted_row.append(self._convert_value(native_value))
    rows.append(converted_row)
```

**Frontend Integration:**

The frontend `parseDateColumns()` utility converts ISO date strings to JavaScript Date objects:

```javascript
// In data source plugins (genericProxyDataSource.js, bigQueryDataSource.js):
const parsedRows = parseDateColumns(columnsMeta, rowData);
```

This uses column type metadata (`type: "date"`, `type: "timestamp"`) to identify which columns need parsing.

**Testing Value Conversion:**

Integration tests should verify proper type conversion:

```python
def test_integer_type(self, provider):
    result = provider.execute_query("SELECT 1 as num")
    assert result.rows[0][0] == 1  # int, not "1"
    assert isinstance(result.rows[0][0], int)

def test_boolean_type(self, provider):
    result = provider.execute_query("SELECT true as flag")
    assert result.rows[0][0] is True  # bool, not "true"
    assert isinstance(result.rows[0][0], bool)

def test_float_type(self, provider):
    result = provider.execute_query("SELECT 3.14 as pi")
    assert abs(result.rows[0][0] - 3.14) < 0.001
    assert isinstance(result.rows[0][0], float)
```

### SSH Tunnel Support (Optional Feature)

**Reference Implementation:** `apps/backend/src/api/datasources/postgres/provider.py`

For databases behind firewalls or in private networks, SSH tunnel support is essential. The PostgreSQL provider demonstrates a complete implementation.

#### When to Implement SSH Tunnels

- Database is in a private network (no public IP)
- Corporate firewall blocks direct database connections
- Security policy requires bastion host access
- Cloud databases in private VPCs

#### Configuration Fields

Add these fields to your `ConnectionConfig` (workspace-level):

```python
class <Type>ConnectionConfig(BaseModel):
    # ... existing fields ...

    # SSH tunnel configuration
    ssh_enabled: bool = Field(False, description="Enable SSH tunnel for connection")
    ssh_host: Optional[str] = Field(None, description="SSH bastion host")
    ssh_port: int = Field(22, description="SSH port (default: 22)")
    ssh_username: Optional[str] = Field(None, description="SSH username")
    # Note: ssh_private_key and ssh_public_key are managed separately via the
    # generate-ssh-key endpoint and stored encrypted in connection_config
```

#### SSH Keypair Generation

**Reference:** `apps/backend/src/api/datasources/postgres/provider.py` (lines 441-473)

Use Ed25519 keys (modern, secure, short public keys):

```python
def generate_ssh_keypair() -> Tuple[str, str]:
    """
    Generate Ed25519 SSH keypair for tunnel authentication.

    Returns:
        Tuple of (private_key_pem, public_key_openssh)
        - private_key_pem: PEM-encoded private key for storage
        - public_key_openssh: OpenSSH format public key for authorized_keys
    """
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    # Generate Ed25519 key pair
    private_key = Ed25519PrivateKey.generate()

    # Serialize private key to PEM format
    private_key_pem = private_key.private_bytes(
        encoding=serialization.Encoding.PEM,
        format=serialization.PrivateFormat.OpenSSH,
        encryption_algorithm=serialization.NoEncryption()
    ).decode("utf-8")

    # Serialize public key to OpenSSH format
    public_key = private_key.public_key()
    public_key_openssh = public_key.public_bytes(
        encoding=serialization.Encoding.OpenSSH,
        format=serialization.PublicFormat.OpenSSH
    ).decode("utf-8")

    # Add a comment to the public key for identification
    public_key_openssh = f"{public_key_openssh} kyomi-datasource"

    return private_key_pem, public_key_openssh
```

#### Tunnel Creation in Provider

**Reference:** `apps/backend/src/api/datasources/postgres/provider.py` (lines 98-139)

```python
def _create_ssh_tunnel(self) -> Tuple[str, int]:
    """
    Create SSH tunnel to bastion host.

    Returns:
        Tuple of (local_bind_host, local_bind_port) to connect through
    """
    from sshtunnel import SSHTunnelForwarder

    ssh_host = self.connection_config.get("ssh_host")
    ssh_port = self.connection_config.get("ssh_port", 22)
    ssh_username = self.connection_config.get("ssh_username")
    ssh_private_key = self.connection_config.get("ssh_private_key")

    # Target database server (from bastion's perspective)
    db_host = self.connection_config.get("host", "localhost")
    db_port = self.connection_config.get("port", 5432)

    if not all([ssh_host, ssh_username, ssh_private_key]):
        raise ValueError(
            "SSH tunnel requires ssh_host, ssh_username, and ssh_private_key"
        )

    logger.info(f"Creating SSH tunnel to {ssh_host}:{ssh_port} -> {db_host}:{db_port}")

    # Create tunnel using private key from string
    self._tunnel = SSHTunnelForwarder(
        (ssh_host, ssh_port),
        ssh_username=ssh_username,
        ssh_pkey=ssh_private_key,  # sshtunnel accepts key as string
        remote_bind_address=(db_host, db_port),
        local_bind_address=("127.0.0.1", 0),  # Random local port
    )

    self._tunnel.start()

    local_host = self._tunnel.local_bind_host
    local_port = self._tunnel.local_bind_port

    logger.info(f"SSH tunnel established: localhost:{local_port} -> {db_host}:{db_port}")

    return local_host, local_port
```

#### Connection Logic with Tunnel

```python
def _get_connection(self):
    """Get or create database connection (through SSH tunnel if configured)."""
    if self._connection is not None and not self._connection.closed:
        return self._connection

    ssh_enabled = self.connection_config.get("ssh_enabled", False)

    if ssh_enabled:
        # Connect through SSH tunnel
        local_host, local_port = self._create_ssh_tunnel()
        # Connect to local tunnel endpoint
        self._connection = create_db_connection(
            host=local_host,
            port=local_port,
            # ... other connection params
        )
    else:
        # Direct connection
        self._connection = create_db_connection(
            host=self.connection_config.get("host"),
            port=self.connection_config.get("port"),
            # ... other connection params
        )

    return self._connection
```

#### Cleanup

Always clean up tunnels in the `close()` method:

```python
def close(self):
    """Clean up database connection and SSH tunnel."""
    if self._connection:
        try:
            self._connection.close()
        except Exception as e:
            logger.warning(f"Error closing connection: {e}")
        self._connection = None

    if self._tunnel:
        try:
            self._tunnel.stop()
        except Exception as e:
            logger.warning(f"Error closing SSH tunnel: {e}")
        self._tunnel = None
```

#### Dependencies

Add to `pyproject.toml`:

```toml
dependencies = [
    "sshtunnel>=0.4.0",
    "cryptography>=41.0.0",  # For Ed25519 key generation
]
```

#### Security Considerations

1. **Private key encryption**: Store `ssh_private_key` in encrypted field (EncryptedJSON)
2. **Public key display**: `ssh_public_key` can be shown to user (not secret)
3. **Key rotation**: Provide UI to regenerate keypair if compromised
4. **Bastion access**: User must add public key to bastion's `~/.ssh/authorized_keys`

#### Frontend UI - Automatic Rendering

**Important:** SSH tunnel UI is **automatically rendered** based on ConnectionConfig fields. You do NOT need to add special registry metadata for SSH tunnel support.

The frontend settings component detects SSH tunnel support by checking if the ConnectionConfig schema includes `ssh_enabled`, `ssh_host`, `ssh_port`, and `ssh_username` fields. When these fields are present, the SSH tunnel configuration section is automatically displayed in the datasource settings modal.

**No code changes needed** - just add the SSH fields to your `<Type>ConnectionConfig` Pydantic model and the UI will adapt automatically.

### Shared Credentials (Optional Feature)

**Reference Implementation:** `apps/backend/src/api/routers/datasources.py` (lines 126-137)

Some databases don't require individual user credentials. Shared credentials allow workspace admins to configure a single set of credentials that all users share.

#### When to Use Shared Credentials

- Database uses service account authentication
- All team members should use same read-only credentials
- Individual user credentials are impractical (cost, management overhead)
- Database doesn't support multiple user accounts

#### Configuration Fields

Add to your `ConnectionConfig` (workspace-level):

```python
class <Type>ConnectionConfig(BaseModel):
    # ... existing fields ...

    # Shared credentials - when enabled, all users use these credentials instead of their own
    shared_credentials: bool = Field(
        False,
        description="If true, all users share the credentials below instead of providing their own"
    )
    shared_username: Optional[str] = Field(
        None,
        description="Shared database username (only used if shared_credentials=true)"
    )
    shared_password: Optional[str] = Field(
        None,
        description="Shared database password - stored encrypted (only used if shared_credentials=true)"
    )
```

#### Provider Logic

Modify credential resolution in your provider:

```python
def __init__(self, connection_config: Dict[str, Any], credentials: Optional[Dict[str, Any]] = None):
    """Initialize provider with connection config and user credentials."""
    self.connection_config = connection_config

    # Use shared credentials if enabled, otherwise user credentials
    if connection_config.get("shared_credentials"):
        self.credentials = {
            "username": connection_config.get("shared_username"),
            "password": connection_config.get("shared_password"),
        }
    else:
        self.credentials = credentials or {}
```

#### UI/UX Considerations

**Settings UI** should show:
- Checkbox: "Use shared credentials for all users"
- When checked: Show username/password fields in Connection tab (admin only)
- When unchecked: Show username/password fields in Credentials tab (all users)

**Permissions:**
- Only workspace admins can toggle `shared_credentials`
- Only workspace admins can edit `shared_username` and `shared_password`
- Regular users see read-only indicator when shared credentials are enabled

#### Security

- `shared_password` MUST be stored in `EncryptedJSON` field
- Never expose shared password in API responses (mask or omit)
- Audit log should track when shared credentials are enabled/changed

### Connection Management Best Practices

**Reference Implementation:** `apps/backend/src/api/datasources/postgres/provider.py`

#### Instance-Level Connection State

Store connection and tunnel state at instance level for reuse:

```python
class <Type>Provider(BaseDatasourceProvider):
    def __init__(self, connection_config: Dict[str, Any], credentials: Dict[str, Any]):
        super().__init__(connection_config, credentials)
        self._connection = None  # Cached connection
        self._tunnel = None      # Cached SSH tunnel (if applicable)
```

#### Connection Reuse with State Checking

```python
def _get_connection(self):
    """Get or create database connection with state checking."""
    # Reuse existing connection if still valid
    if self._connection is not None and not self._connection.closed:
        return self._connection

    # Create new connection
    self._connection = self._create_connection()
    return self._connection
```

#### Context Manager Support

Implement context manager for automatic cleanup:

```python
def __enter__(self):
    """Context manager entry."""
    return self

def __exit__(self, exc_type, exc_val, exc_tb):
    """Context manager exit - ensure cleanup."""
    self.close()
    return False  # Don't suppress exceptions
```

**Usage:**

```python
with <Type>Provider(config, credentials) as provider:
    result = provider.execute_query("SELECT * FROM table")
    # Connection automatically closed on exit
```

#### Robust Cleanup

Handle errors during cleanup gracefully:

```python
def close(self):
    """Clean up all resources with error handling."""
    if self._connection:
        try:
            self._connection.close()
        except Exception as e:
            logger.warning(f"Error closing connection: {e}")
        finally:
            self._connection = None  # Always clear reference

    if self._tunnel:
        try:
            self._tunnel.stop()
        except Exception as e:
            logger.warning(f"Error stopping tunnel: {e}")
        finally:
            self._tunnel = None  # Always clear reference
```

### Query Pagination Support (Recommended)

**Reference Implementation:** `apps/backend/src/api/datasources/postgres/provider.py` (lines 209-320)

Proper pagination support improves performance and enables large result sets.

#### Execute Query Signature

```python
def execute_query(
    self,
    sql: str,
    limit: int = 1000,
    offset: int = 0,
    dry_run: bool = False,
    include_total: bool = True,
) -> QueryResult:
    """
    Execute SQL query with pagination support.

    Args:
        sql: SQL query to execute
        limit: Maximum rows to return (page size)
        offset: Number of rows to skip
        dry_run: If True, validate query without executing (EXPLAIN)
        include_total: If True, include total row count (may be slow for large tables)

    Returns:
        QueryResult with columns, rows, and pagination metadata
    """
```

#### Total Row Count

Execute separate COUNT query to get total rows:

```python
# Get total count if requested (before pagination)
total_rows = None
if include_total:
    try:
        count_sql = f"SELECT COUNT(*) FROM ({sql}) AS _count_subquery"
        with conn.cursor() as cursor:
            cursor.execute(count_sql)
            count_result = cursor.fetchone()
            total_rows = count_result[0] if count_result else None
    except Exception as e:
        # If COUNT fails, rollback to clear the aborted transaction state
        # then continue without total count
        logger.warning(f"Failed to get total count: {e}")
        conn.rollback()
        total_rows = None
```

#### Intelligent LIMIT Handling

Check if query already has LIMIT before adding:

```python
# Remove trailing semicolon and add LIMIT/OFFSET
sql_stripped = sql.rstrip(";").strip()
sql_upper = sql_stripped.upper()

# Check if query already has LIMIT/OFFSET
if "LIMIT" not in sql_upper:
    paginated_sql = f"{sql_stripped} LIMIT {limit} OFFSET {offset}"
else:
    # Query already has LIMIT, use as-is (respect user intent)
    paginated_sql = sql_stripped
```

#### Pagination Metadata

Return pagination info in QueryResult:

```python
return QueryResult(
    status="success",
    columns=columns,
    rows=rows,
    total_rows=total_rows,           # Total rows (from COUNT query)
    has_more=total_rows > (offset + len(rows)) if total_rows else False,
    offset=offset,
    limit=limit,
)
```

#### Performance Considerations

- `include_total=False` for better performance when total count isn't needed
- COUNT queries can be slow on large tables (millions of rows)
- Consider caching total counts for frequently-accessed tables
- Some databases have efficient alternatives (e.g., PostgreSQL `EXPLAIN` for estimates)

### SQL Validation Service Integration

All datasource providers automatically integrate with the centralized SQL validation service once they implement the `dry_run()` method.

**Service Location:** `apps/backend/src/api/services/sql_validation_service.py`

**Integration Points:**

The SQL validation service is used by:
1. **SQL Editor** - Real-time validation as users type
2. **ChartML Validation** - Validates SQL in ChartML `data.query` blocks
3. **Agent Tools** - `validate_sql` tool for AI agents
4. **SQL Copilot** - Validates generated SQL before returning to user

**No Additional Work Required:**

Once your provider implements `dry_run(sql) -> DryRunResult`, all these integration points automatically work. The service:
1. Resolves the datasource by slug
2. Creates a provider instance
3. Calls `provider.dry_run(sql)`
4. Returns the `DryRunResult` to the caller

**Testing Your Integration:**

After implementing `dry_run()`, test via the SQL Editor:
1. Create a datasource of your type in Settings
2. Go to SQL Editor and select your datasource
3. Type an invalid query - you should see error with line number
4. Type a valid query - you should see "Query valid"

### 3. Catalog Indexer

**Create:** `apps/backend/src/api/datasources/<type>/indexer.py`

For SQL-based datasources, extend `BaseSQLCatalogIndexer` which provides all common functionality. You only need to implement 5 abstract methods (~50 lines instead of ~700).

**Reference:** `apps/backend/src/api/datasources/base_sql_indexer.py` - Base class source code

```python
from ..base_sql_indexer import BaseSQLCatalogIndexer
from ..base import BaseDatasourceProvider
from .provider import <Type>Provider
from typing import Dict, Any, List, Optional
import logging

logger = logging.getLogger(__name__)

class <Type>CatalogIndexer(BaseSQLCatalogIndexer):
    """
    Catalog indexer for <Type> datasources.

    Extends BaseSQLCatalogIndexer which handles:
    - Workspace status updates
    - Credential fetching
    - Table caching with embeddings
    - Archive detection
    - Rate limiting

    This class only implements provider-specific discovery methods.
    """

    # Required properties
    container_label = "database"  # or "schema" for PostgreSQL-like
    container_config_key = "catalog_databases"  # or "catalog_schemas"

    # Required abstract method implementations:

    def _create_provider(self, credentials: Dict[str, Any]) -> BaseDatasourceProvider:
        """Create provider instance for this datasource type."""
        return <Type>Provider(
            connection_config=self.connection_config,
            credentials=credentials
        )

    def _get_catalog_containers(self, provider: BaseDatasourceProvider) -> List[str]:
        """
        Get list of containers to index (schemas or databases).

        Should respect self.connection_config.get(self.container_config_key) if configured.
        Should exclude system containers.
        """
        configured = self.connection_config.get(self.container_config_key) or []
        if configured:
            return configured

        # Query to list non-system containers
        result = provider.execute_query("""
            SELECT schema_name FROM information_schema.schemata
            WHERE schema_name NOT IN ('pg_catalog', 'information_schema')
        """)
        return [row[0] for row in result.rows] if result.rows else []

    def _get_tables_in_container(
        self,
        provider: BaseDatasourceProvider,
        container_name: str,
        max_tables: Optional[int]
    ) -> List[Dict[str, str]]:
        """Get tables in a container."""
        container_escaped = container_name.replace("'", "''")
        limit = f"LIMIT {max_tables}" if max_tables else ""

        result = provider.execute_query(f"""
            SELECT table_name, table_type
            FROM information_schema.tables
            WHERE table_schema = '{container_escaped}'
            {limit}
        """)
        return [{"name": row[0], "type": row[1]} for row in result.rows] if result.rows else []

    def _get_table_columns(
        self,
        provider: BaseDatasourceProvider,
        container_name: str,
        table_name: str
    ) -> List[Dict[str, Any]]:
        """Get column metadata for a table."""
        container_escaped = container_name.replace("'", "''")
        table_escaped = table_name.replace("'", "''")

        result = provider.execute_query(f"""
            SELECT column_name, data_type
            FROM information_schema.columns
            WHERE table_schema = '{container_escaped}' AND table_name = '{table_escaped}'
        """)
        return [
            {"name": row[0], "type": self._map_type(row[1]), "native_type": row[1], "description": ""}
            for row in result.rows
        ] if result.rows else []

    def _map_type(self, native_type: str) -> str:
        """Map native type to simple type (number, string, date, etc.)."""
        # Implement type mapping for your database
        pass
```

**The base class provides these methods (do NOT override):**
- `index_catalog()` - Main entry point with full workflow
- `can_refresh_now()` - Rate limiting check
- `get_last_refresh_time()` - Last refresh timestamp
- `_get_stored_credentials()` - Fetch credentials from database
- `_has_valid_credentials()` - Delegates to provider's `credentials_are_configured()`
- `_cache_table()` - Store table with embeddings
- `_archive_missing_tables()` - Mark deleted tables as archived
- `_create_search_entries()` - Generate weighted search entries
- `_update_workspace_status()` - Update workspace status
- `_update_workspace_last_refresh()` - Update last refresh timestamp

### Credential Validation via Provider (Required)

**Architecture Principle: The provider owns credential validation.**

Each provider implements a class method `credentials_are_configured()` that validates credentials.
The indexer delegates to the provider via `_get_provider_class()`:

```python
# In BaseSQLCatalogIndexer (do NOT override)
def _has_valid_credentials(self, credentials: Dict[str, Any]) -> bool:
    """Delegates to the provider class."""
    provider_class = self._get_provider_class()
    return provider_class.credentials_are_configured(credentials)
```

**Required Abstract Methods (implement these in your indexer):**

```python
class <Type>CatalogIndexer(BaseSQLCatalogIndexer):
    container_label = "database"  # or "schema"
    container_config_key = "catalog_databases"  # or "catalog_schemas"

    def _get_provider_class(self) -> Type[BaseDatasourceProvider]:
        """Return the provider class for credential validation."""
        return <Type>Provider  # The class itself, not an instance

    def _create_provider(self, credentials: Dict[str, Any]) -> BaseDatasourceProvider:
        """Create provider instance for catalog discovery."""
        return <Type>Provider(
            connection_config=self.connection_config,
            credentials=credentials
        )

    # ... other abstract methods
```

**Provider must implement `credentials_are_configured()`:**

```python
class <Type>Provider(BaseDatasourceProvider):

    @classmethod
    def credentials_are_configured(cls, credentials: Dict[str, Any]) -> bool:
        """
        Check if minimum required credentials are present.

        This is a CLASS METHOD - no instance needed.
        """
        if not credentials:
            return False
        # Example for username/password auth:
        return bool(credentials.get("username"))

        # Example for token-based auth (Databricks):
        # if credentials.get("access_token"):
        #     return True
        # if credentials.get("client_id") and credentials.get("client_secret"):
        #     return True
        # return False
```

**Common Authentication Patterns:**

| Datasource | Auth Type | Credential Fields | Implementation |
|------------|-----------|-------------------|----------------|
| PostgreSQL | Username/Password | `username` | `bool(credentials.get("username"))` |
| MySQL | Username/Password | `username` | `bool(credentials.get("username"))` |
| ClickHouse | Username/Password | `username` | `bool(credentials.get("username"))` |
| Snowflake | Password OR Key-pair | `username` + (`password` OR `private_key`) | Check username AND (password OR private_key) |
| Databricks | Token OR OAuth | `access_token` OR (`client_id` + `client_secret`) | Check either pattern |
| Redshift | Password OR IAM | `username` OR (`iam` + `cluster_identifier` + `db_user`) | Check based on `iam` flag |
| BigQuery | OAuth | `billing_project` | Check billing_project (OAuth handled separately) |

**Why This Architecture?**

1. **Single source of truth** - Credential validation logic lives in the provider
2. **No assumptions** - Base class doesn't assume any particular auth pattern
3. **Self-documenting** - Reading the provider tells you what credentials it needs
4. **Easy to extend** - New datasources just implement one method in the provider
5. **Testable** - Class method can be tested without instantiating provider

**Reference Document:** See `docs/specifications/CREDENTIAL_VALIDATION_REFACTOR.md` for the full architecture decision record.

**Common System Catalog Queries:**
| Datasource | Databases/Schemas | Tables | Columns |
|------------|------------------|--------|---------|
| PostgreSQL | `information_schema.schemata` | `information_schema.tables` | `information_schema.columns` |
| MySQL | `information_schema.schemata` | `information_schema.tables` | `information_schema.columns` |
| ClickHouse | `system.databases` | `system.tables` | `system.columns` |

### 4. Register Provider with Metadata (CRITICAL)

**Modify:** `apps/backend/src/api/datasources/<type>/__init__.py`

This is the **single source of truth** for your datasource type. All routing decisions read from this metadata.

```python
"""
<Type> Datasource Provider

<Brief description of the datasource and any special features>
"""

from ..registry import DatasourceTypeRegistry, DatasourceTypeMetadata

# Self-register on import - eliminates all switch statements!
DatasourceTypeRegistry.register(DatasourceTypeMetadata(
    # === Identity ===
    type_id="<type>",                         # Internal identifier (lowercase, no spaces)
    display_name="<Type>",                    # Human-readable name for UI
    description="<Type> database server",     # Brief description

    # === Provider Configuration ===
    provider_class_name="<Type>Provider",     # Class name for lazy import
    requires_user_credentials=True,           # False only for OAuth (BigQuery)
    accepts_user_context=False,               # True only for OAuth (BigQuery)

    # === Indexer Configuration ===
    indexer_class_name="<Type>CatalogIndexer",

    # === Catalog Hierarchy ===
    # How tables are organized in this datasource
    catalog_container_key="catalog_databases",     # or "catalog_schemas" for PostgreSQL
    catalog_container_label="database",            # or "schema" for UI labels

    # === Catalog Tree Rendering ===
    # Controls how the catalog tree is displayed in the UI
    tree_level1_type=None,                    # "project", "database", or None to skip
    tree_level2_type="database",              # "dataset", "schema", "database"
    skip_empty_project_wrapper=True,          # True for MySQL/ClickHouse (no project)
    skip_single_project_wrapper=False,        # True for PostgreSQL (single database)

    # === Credential Configuration ===
    credential_validator_name="<Type>Credentials",         # Pydantic model name
    credential_fields=["username", "password"],            # Fields in credentials
    sensitive_credential_fields=["password"],              # Fields to mask

    # === Connection Configuration ===
    connection_validator_name="<Type>ConnectionConfig",   # Pydantic model name
    default_port=3306,                                     # Default port number

    # === Discovery ===
    supports_catalog_discovery=True,
    discovery_method="list_databases",        # or "list_schemas" for PostgreSQL

    # === Catalog Status ===
    catalog_config_keys=["catalog_databases"],  # Keys to extract for status endpoint
))

# Import after registration
try:
    from .provider import <Type>Provider
    from .indexer import <Type>CatalogIndexer
    __all__ = ["<Type>Provider", "<Type>CatalogIndexer"]
except ImportError:
    # Optional dependencies not installed
    __all__ = []
```

**Copy-Paste Template (ALL fields with defaults explicitly shown):**

```python
from ..registry import DatasourceTypeRegistry, DatasourceTypeMetadata

# Self-register on import
DatasourceTypeRegistry.register(DatasourceTypeMetadata(
    # === Identity === (REQUIRED)
    type_id="<type>",                                   # e.g., "mysql", "clickhouse", "snowflake"
    display_name="<Display Name>",                      # e.g., "MySQL", "ClickHouse"
    description="<Brief description>",                  # e.g., "MySQL database server"

    # === Provider Configuration === (REQUIRED)
    provider_class_name="<Type>Provider",               # e.g., "MySQLProvider"
    requires_user_credentials=True,                     # False only for OAuth (BigQuery)
    accepts_user_context=False,                         # True only for OAuth (BigQuery)

    # === Indexer Configuration === (REQUIRED)
    indexer_class_name="<Type>CatalogIndexer",         # e.g., "MySQLCatalogIndexer"

    # === Catalog Hierarchy === (REQUIRED)
    catalog_container_key="catalog_<containers>",       # e.g., "catalog_databases" or "catalog_schemas"
    catalog_container_label="<container>",              # e.g., "database", "schema", "project"

    # === Catalog Tree Rendering === (REQUIRED)
    tree_level1_type=None,                              # "project", "database", or None
    tree_level2_type="<container>",                     # "dataset", "schema", or "database"
    skip_empty_project_wrapper=True,                    # True for MySQL/ClickHouse, False for BigQuery
    skip_single_project_wrapper=False,                  # True for PostgreSQL, False otherwise

    # === Credential Configuration === (REQUIRED)
    credential_validator_name="<Type>Credentials",     # e.g., "MySQLCredentials"

    # === Connection Configuration === (REQUIRED)
    connection_validator_name="<Type>ConnectionConfig", # e.g., "MySQLConnectionConfig"

    # === Optional Fields (explicitly showing defaults) ===
    credential_fields=["username", "password"],         # Default: [] - List credential field names
    sensitive_credential_fields=["password"],           # Default: [] - Fields to mask in API
    default_port=3306,                                  # Default: None - Default port number
    supports_catalog_discovery=True,                    # Default: True - Can list schemas/databases
    discovery_method="list_databases",                  # Default: "list_schemas" - Provider method name
    catalog_config_keys=["catalog_databases"],          # Default: [] - Keys for /catalog/status
))

# Import after registration (with optional try/except for missing dependencies)
try:
    from .provider import <Type>Provider
    from .indexer import <Type>CatalogIndexer
    __all__ = ["<Type>Provider", "<Type>CatalogIndexer"]
except ImportError:
    # Optional dependencies not installed
    __all__ = []
```

**SSH Tunnel & Shared Credentials Support:**

The registry metadata doesn't have explicit `supports_ssh_tunnel` or `supports_shared_credentials` fields. These features are **detected dynamically from ConnectionConfig fields**:

- **SSH Tunnel**: Detected when ConnectionConfig has `ssh_enabled`, `ssh_host`, `ssh_port`, `ssh_username` fields
- **Shared Credentials**: Detected when ConnectionConfig has `shared_credentials`, `shared_username`, `shared_password` fields

The frontend UI renders SSH tunnel and shared credentials sections automatically when these fields are present in the connection schema. See [SSH Tunnel Support](#ssh-tunnel-support-optional-feature) and [Shared Credentials](#shared-credentials-optional-feature) sections for implementation details.

**Field Guide - Complete DatasourceTypeMetadata Fields:**

| Field | Required | Purpose | Examples |
|-------|----------|---------|----------|
| `type_id` | ✅ Yes | Internal identifier, must match enum | `"mysql"`, `"postgres"`, `"clickhouse"` |
| `display_name` | ✅ Yes | Human-readable name shown in UI | `"MySQL"`, `"PostgreSQL"` |
| `description` | ✅ Yes | Brief description for UI tooltips | `"MySQL database server"` |
| `provider_class_name` | ✅ Yes | Class name for lazy import | `"MySQLProvider"`, `"PostgresProvider"` |
| `requires_user_credentials` | ✅ Yes | True if users provide username/password | `True` for MySQL/PostgreSQL, `False` for BigQuery |
| `accepts_user_context` | ✅ Yes | True if provider needs user context (OAuth) | `True` only for BigQuery, `False` for others |
| `indexer_class_name` | ✅ Yes | Indexer class name for lazy import | `"MySQLCatalogIndexer"` |
| `catalog_container_key` | ✅ Yes | Config key for catalog containers | `"catalog_databases"`, `"catalog_schemas"` |
| `catalog_container_label` | ✅ Yes | UI label for containers (singular) | `"database"`, `"schema"`, `"project"` |
| `tree_level1_type` | ✅ Yes | Top-level catalog node type (or None) | `"project"` (BigQuery), `"database"` (PostgreSQL), `None` (MySQL) |
| `tree_level2_type` | ✅ Yes | Second-level catalog node type | `"dataset"` (BigQuery), `"schema"` (PostgreSQL), `"database"` (MySQL) |
| `skip_empty_project_wrapper` | ✅ Yes | Skip level1 when project_id is empty | `True` for MySQL/ClickHouse, `False` for BigQuery |
| `skip_single_project_wrapper` | ✅ Yes | Skip level1 when only one database | `True` for PostgreSQL, `False` for BigQuery/MySQL |
| `credential_validator_name` | ✅ Yes | Pydantic model name for validating credentials | `"MySQLCredentials"`, `"PostgresCredentials"` |
| `connection_validator_name` | ✅ Yes | Pydantic model name for connection config | `"MySQLConnectionConfig"` |
| `credential_fields` | Optional | List of credential field names | `["username", "password"]` (default: `[]`) |
| `sensitive_credential_fields` | Optional | List of fields to mask in API responses | `["password"]` (default: `[]`) |
| `default_port` | Optional | Default port number for this database | `3306` for MySQL, `5432` for PostgreSQL, `None` for BigQuery |
| `supports_catalog_discovery` | Optional | Whether provider can list schemas/databases | `True` (default), `False` for restricted providers |
| `discovery_method` | Optional | Provider method name for listing containers | `"list_databases"`, `"list_schemas"` (default: `"list_schemas"`) |
| `catalog_config_keys` | Optional | Keys to extract from connection_config for status | `["catalog_databases"]` (default: `[]`) |

**Why This Matters:**

With registry-based registration:
- ✅ No switch statements to update in routers
- ✅ No manual registration in multiple files
- ✅ All metadata in one place
- ✅ Frontend can query `/api/v1/datasources/types` for dynamic UI

**Important: Table ID Construction for `tree_level1_type=None`**

When your datasource has `tree_level1_type=None` (like MySQL/ClickHouse), the project/database is an empty string. The catalog tree building code handles this automatically:

| `tree_level1_type` | Table ID Format | Example |
|--------------------|-----------------|---------|
| `"project"` (BigQuery) | `project.dataset.table` | `my-project.analytics.users` |
| `"database"` (PostgreSQL) | `database.schema.table` | `mydb.public.users` |
| `None` (MySQL/ClickHouse) | `database.table` | `test_db.customers` |

The code skips the empty project prefix to avoid malformed IDs like `.test_db.customers`.

### 5. Register Catalog Indexer

**No action required!** The registry automatically finds your indexer via the metadata you registered in step 4.

The `CatalogIndexingService` uses `DatasourceTypeRegistry.get_indexer_class(type_id)` which lazily imports your indexer class.

**Verification Steps - Confirm Registration Worked:**

After completing step 4 (registry registration), verify your datasource type is correctly registered:

```python
# From Python shell or test script
from api.datasources.registry import DatasourceTypeRegistry

# 1. Verify type is registered
assert DatasourceTypeRegistry.is_registered("<type>")
print("✓ Type registered")

# 2. Verify metadata is accessible
meta = DatasourceTypeRegistry.get("<type>")
print(f"✓ Metadata: {meta.display_name}")

# 3. Test provider class lazy loading
ProviderClass = DatasourceTypeRegistry.get_provider_class("<type>")
print(f"✓ Provider class loaded: {ProviderClass.__name__}")

# 4. Test indexer class lazy loading
IndexerClass = DatasourceTypeRegistry.get_indexer_class("<type>")
print(f"✓ Indexer class loaded: {IndexerClass.__name__}")

# 5. Verify tree level types
level1, level2 = DatasourceTypeRegistry.get(type_id="<type>").tree_level1_type, DatasourceTypeRegistry.get(type_id="<type>").tree_level2_type
print(f"✓ Tree levels: {level1} > {level2}")
```

**Common Issues:**

| Error | Cause | Fix |
|-------|-------|-----|
| `ValueError: Unknown datasource type` | Type not registered | Check package `__init__.py` is imported on app startup |
| `ImportError: Failed to import provider` | Missing dependencies | Install required packages (e.g., `clickhouse-driver`) |
| `ImportError: Provider class not found` | Wrong class name in metadata | Verify `provider_class_name` matches actual class name |
| `AttributeError: module has no attribute` | Import order issue | Ensure registration happens before imports in `__init__.py` |

### 6. Add Connection Config Validator

**Modify:** `apps/backend/src/api/schemas/datasource_schemas.py`

Add Pydantic model for connection validation:

```python
class <Type>ConnectionConfig(BaseModel):
    """Validation schema for <Type> connection configuration."""
    host: str
    port: int = <default_port>
    database: str
    # Add other required/optional fields

    class Config:
        extra = "forbid"  # Reject unknown fields
```

Add to the `CONFIG_VALIDATORS` dict:

```python
CONFIG_VALIDATORS = {
    "bigquery": BigQueryConnectionConfig,
    "postgres": PostgresConnectionConfig,
    "<type>": <Type>ConnectionConfig,  # Add this
}
```

Also add `<Type>Credentials` Pydantic model for credential validation in the same file.

### 6.1 Provider Class Registration

**No action required!** The registry automatically finds your provider via the metadata you registered in step 4.

The `DatasourceProviderService` uses `DatasourceTypeRegistry.get_provider_class(type_id)` which lazily imports your provider class.

**Old PROVIDER_REGISTRY removed** - replaced by the unified registry system.

### 6.2 Add Catalog Endpoint Support

**Minimal changes required!** The registry eliminates most switch statements.

**Note:** Catalog endpoints are in `apps/backend/src/api/routers/catalog.py` (extracted from datasources.py).

#### A. `/schemas` endpoint - Discovery method routing

The router uses the registry to find the correct discovery method:

```python
# Router code (already implemented) - NO CHANGES NEEDED
meta = DatasourceTypeRegistry.get(datasource.datasource_type)
discovery_method = getattr(provider, meta.discovery_method)
discovery = discovery_method()
schemas = discovery.items
discovery_error = discovery.error
```

**Your provider must implement the method named in `discovery_method`:**
- `list_databases()` for MySQL/ClickHouse
- `list_schemas()` for PostgreSQL
- `list_projects()` for BigQuery

#### B. `/catalog/status` endpoint - Catalog config extraction

The router uses the registry to extract catalog config:

```python
# Router code (already implemented) - NO CHANGES NEEDED
meta = DatasourceTypeRegistry.get(datasource.datasource_type)
catalog_config = {
    key: conn_config.get(key, [] if key != "include_public_datasets" else False)
    for key in meta.catalog_config_keys
}
```

**Ensure your metadata includes the correct `catalog_config_keys`:**
- `["catalog_databases"]` for MySQL/ClickHouse
- `["catalog_schemas"]` for PostgreSQL
- `["catalog_projects", "include_public_datasets"]` for BigQuery

#### Provider Method Requirements

Your provider should implement one of these methods:

```python
def list_databases(self) -> DiscoveryResult:
    """For database-centric datasources (MySQL, ClickHouse)."""
    # Query information_schema or system tables
    return DiscoveryResult(items=["db1", "db2"], error=None)

def list_schemas(self) -> DiscoveryResult:
    """For schema-centric datasources (PostgreSQL)."""
    # Query information_schema.schemata
    return DiscoveryResult(items=["public", "myschema"], error=None)
```

### 7. Agent Integration

**No code changes required!** The agent tools are datasource-agnostic.

**File:** `apps/backend/src/api/agent/chat_agent_adapter.py`

#### How It Works

The agent tools use the **provider pattern** via the registry:

1. Agent calls `query_datasource(datasource_slug, sql)`
2. Router resolves datasource by slug → gets `datasource_type`
3. `DatasourceProviderService` uses registry → `get_provider_class(datasource_type)`
4. Provider instance created → `execute_query(sql)` called
5. Results returned to agent

**As long as your provider implements `BaseDatasourceProvider`, no agent changes are needed.**

#### Required Provider Methods

Your provider must implement:
- `execute_query(sql, limit)` → `QueryResult`
- `get_table_info(table_name)` → `Dict[str, Any]`
- `dry_run(sql)` → `DryRunResult`
- `test_connection()` → `bool`

#### SQL Dialect Learning

The agent learns SQL dialects from:
- Datasource type in search results (`datasource_type` field)
- System catalog info from `get_table_info()`
- Error messages from failed queries

No special configuration needed - the agent adapts automatically.

### 8. Add Dependencies (if needed)

**Modify:** `apps/backend/pyproject.toml`

```toml
dependencies = [
    # ... existing deps ...
    "<driver-package>>=x.y.z",  # e.g., "clickhouse-driver>=0.2.0"
]
```

### 9. Frontend UI

**Modify:** `apps/frontend/src/components/settings/DatasourceSettings.jsx`

**Future Enhancement:** The frontend will eventually query `GET /api/v1/datasources/types` to dynamically render forms based on registry metadata, eliminating the need for hardcoded type lists and form conditionals. For now, manual updates are required.

Add your datasource type to:

1. **Type metadata:**
```javascript
const DATASOURCE_TYPES = {
  bigquery: { label: 'BigQuery', description: 'Google Cloud BigQuery' },
  postgres: { label: 'PostgreSQL', description: 'PostgreSQL database' },
  '<type>': { label: '<Display Name>', description: '<Description>' },  // Add this
};
```

2. **Connection config form:**
```javascript
{datasourceType === '<type>' && (
  <div className="space-y-4">
    <div>
      <Label>Host</Label>
      <Input
        value={connectionConfig.host || ''}
        onChange={(e) => setConnectionConfig({...connectionConfig, host: e.target.value})}
      />
    </div>
    {/* Add other fields */}
  </div>
)}
```

3. **Credentials form:**
```javascript
{datasourceType === '<type>' && (
  <div className="space-y-4">
    <div>
      <Label>Username</Label>
      <Input value={credentials.username || ''} onChange={...} />
    </div>
    <div>
      <Label>Password</Label>
      <Input type="password" value={credentials.password || ''} onChange={...} />
    </div>
  </div>
)}
```

4. **Catalog configuration (CRITICAL for schema browser):**

Each datasource type needs catalog configuration UI to specify what to index:

| Datasource | Catalog Config Field | Purpose |
|------------|---------------------|---------|
| BigQuery | `catalog_projects` | Which GCP projects to index |
| PostgreSQL | `catalog_schemas` | Which schemas to index (default: exclude `pg_catalog`, `information_schema`) |
| ClickHouse | `catalog_databases` | Which databases to index |
| Snowflake | `catalog_databases` | Which databases/schemas to index |

```javascript
{/* Catalog Configuration - add after connection config */}
<div className="border-t border-border pt-4 mt-4">
  <h4 className="text-sm font-medium text-foreground mb-2">Catalog Indexing</h4>
  <p className="text-xs text-muted-foreground mb-4">
    Select which schemas to include in the catalog for search and browsing.
  </p>

  {/* Schema selector - fetch available schemas after connection test */}
  <div className="space-y-2">
    {(formData.connection_config.catalog_schemas || []).map((schema, idx) => (
      <div key={idx} className="flex items-center gap-2">
        <span className="text-sm">{schema}</span>
        <Button size="sm" variant="ghost" onClick={() => removeSchema(idx)}>
          <X className="h-4 w-4" />
        </Button>
      </div>
    ))}
    <Select onValueChange={addSchema}>
      <SelectTrigger><SelectValue placeholder="Add schema to index..." /></SelectTrigger>
      <SelectContent>
        {availableSchemas.map(s => (
          <SelectItem key={s} value={s}>{s}</SelectItem>
        ))}
      </SelectContent>
    </Select>
  </div>
</div>
```

**Architecture Decision:** The catalog tree will use the indexed `DatasourceTableCache` data via a unified backend endpoint:
- Backend: `GET /api/v1/datasources/{id}/catalog/tree` returns tree structure from cached data
- Frontend: Generic `DatasourceCatalogTree.jsx` component replaces BigQuery-specific tree
- All datasource types use the same tree component and endpoint
- Tree hierarchy adapts to datasource type (BigQuery: project > dataset > table, PostgreSQL: schema > table)

5. **Datasource Icon:**

Each datasource type should have a recognizable icon for quick visual identification.

**Modify:** `apps/frontend/src/components/ui/DatasourceIcon.jsx`

Icons must match Lucide style for consistency:
- 24x24 viewBox
- 2px stroke width
- `currentColor` for theming (grayscale, inherits text color)
- Round linecap and linejoin

Add your icon component and a case to the switch statement:

```javascript
/**
 * <Type> icon - <Description>
 * Matches Lucide style: 24x24, 2px stroke, round caps
 */
const <Type>Icon = ({ className }) => (
  <svg
    className={className}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    {/* Your simplified brand-recognizable paths here */}
  </svg>
);

// Add to DATASOURCE_TYPE_INFO:
export const DATASOURCE_TYPE_INFO = {
  // ...existing types
  <type>: { label: '<Label>', description: '<Description>' },
};

// Add case to DatasourceIcon switch:
export function DatasourceIcon({ type, className = "h-5 w-5" }) {
  switch (type) {
    // ...existing cases
    case '<type>':
      return <<Type>Icon className={className} />;
    default:
      return <Database className={className} />;
  }
}
```

**Design Tips:**
- Create simplified, recognizable versions of brand logos (not exact copies)
- Use stroke paths, not filled shapes
- Keep paths simple - 4-6 path elements max
- Test at small sizes (h-4 w-4) to ensure readability

### 10. ChartML Data Source Plugin (CRITICAL for Dashboards)

**Without this step, dashboards will NOT work for your datasource type!**

ChartML uses a plugin architecture for data sources. For backend-proxied datasources (all except BigQuery), we use a **generic factory** that eliminates code duplication.

#### 10.1 Add Datasource to Generic Proxy Factory (SIMPLE!)

**No new file needed!** Just update the existing factory.

**Modify:** `apps/frontend/src/lib/chartml/plugins/genericProxyDataSource.js`

```javascript
// 1. Add your type to PROXY_DATASOURCES array:
const PROXY_DATASOURCES = [
  'postgres',
  'mysql',      // <-- Add your new type here
  'clickhouse',
  'snowflake',
  'databricks',
  'redshift'
];

// 2. Add error message enhancements (optional but recommended):
const ERROR_ENHANCEMENTS = {
  // ... existing entries ...
  mysql: {  // <-- Add your type's error mappings
    'credentials not configured': 'MySQL Error: Credentials not configured. Please go to Settings → Datasources.',
    'Access denied': 'MySQL Error: Authentication failed. Check username and password.',
    'Unknown database': 'MySQL Error: Database not found. Check database name.',
  },
};

// 3. Add named export for backwards compatibility (optional):
export const mysqlDataSource = createProxyDataSource('mysql');
```

That's it! The factory handles everything else:
- Calls the generic backend endpoint `/api/v1/datasources/query/execute`
- Parses response and converts row format
- Applies error message enhancements
- Auto-registers with ChartML global registry

**When NOT to use genericProxyDataSource:**

The generic proxy factory is appropriate for **credential-based datasources** (username/password). It is NOT appropriate for **OAuth-based datasources** like BigQuery.

OAuth-based datasources need custom plugins because:
1. OAuth tokens are managed client-side (not sent to backend)
2. Direct API calls from browser (no backend proxy)
3. Custom error handling for OAuth token refresh
4. Different query execution patterns

If your datasource uses OAuth (like BigQuery), you'll need to create a custom ChartML plugin file instead. See `apps/frontend/src/lib/chartml/plugins/bigqueryDataSource.js` for reference.

#### 10.2 No Registration Needed in createKyomiChartML.js

The `genericProxyDataSource.js` module auto-registers all proxy datasources when imported. No changes needed to `createKyomiChartML.js`.

#### 10.3 Update ChartML JSON Schema (CRITICAL)

**Without this, ChartML validation will REJECT your datasource type!**

**Modify:** `docs/chartml-spec/chartml_schema.json`

The JSON schema has `provider` enums that must include your new type. There are TWO places to update:

1. **Source definition** (around line 42):
```json
"provider": { "enum": ["bigquery", "clickhouse", "postgres", "snowflake", "databricks", "redshift", "inline", "http"] }
```

2. **Inline data definition** (around line 454):
```json
"provider": { "enum": ["bigquery", "clickhouse", "postgres", "snowflake", "databricks", "redshift", "inline", "http"] }
```

After editing, regenerate the minified schema:
```bash
cd docs/chartml-spec && cat chartml_schema.json | jq -c > chartml_schema.min.json
```

#### 10.4 Update ChartML Specification Docs

**Modify:** `docs/chartml-spec/SPECIFICATION.md`

Add `<type>` to the list of supported providers in TWO places:

1. **Source structure** (around line 82):
```
provider: bigquery | clickhouse | postgres | snowflake | databricks | redshift | inline | http
```

2. **Inline data structure** (around line 360):
```
provider: bigquery | clickhouse | postgres | snowflake | databricks | redshift | inline | http
```

Note that ChartML uses `datasource` (slug) for user-facing references:

```yaml
# Recommended: Use datasource slug
data:
  datasource: "my-<type>-db"   # User-friendly slug
  query: "SELECT * FROM table"

# Alternative: Provider shorthand (when only one datasource of this type)
data:
  provider: <type>
  query: "SELECT * FROM table"
```

### 11. Database Migration (if needed)

**Modify:** `apps/backend/src/api/database/migrations/add_datasource_tables.sql`

The `datasource_type` column uses a PostgreSQL enum type. If adding a new datasource type, you need to alter the enum:

```sql
-- Add new value to existing enum type
ALTER TYPE datasource_type ADD VALUE '<type>';

-- Example:
ALTER TYPE datasource_type ADD VALUE 'mysql';
```

**Important Notes:**

1. **Enum values are permanent** - PostgreSQL does NOT support removing enum values once added
2. **Run migration carefully** - Enum alterations must be done in a separate transaction from other DDL
3. **Case sensitive** - Enum values are case-sensitive, use lowercase (e.g., 'mysql', not 'MySQL')
4. **Check existing values first** - Query existing enum values before adding:
   ```sql
   SELECT enumlabel FROM pg_enum WHERE enumtypid = 'datasource_type'::regtype;
   ```

**Alternative Migration Approach:**

If you need more flexibility (ability to remove types), consider migrating from enum to a varchar check constraint:

```sql
-- NOT RECOMMENDED unless you need to remove types frequently
-- Requires full table rewrite and downtime
ALTER TABLE datasource_config ALTER COLUMN datasource_type TYPE varchar(50);
ALTER TABLE datasource_config ADD CONSTRAINT datasource_type_check
  CHECK (datasource_type IN ('bigquery', 'postgres', 'mysql', ...));
```

## Checklist for New Datasource Types

### Backend (Required)
- [ ] Create `datasources/<type>/__init__.py` with registry metadata
  - [ ] Import and call `DatasourceTypeRegistry.register()`
  - [ ] Fill in all `DatasourceTypeMetadata` fields
  - [ ] Import provider and indexer classes
- [ ] Create `datasources/<type>/provider.py` implementing `BaseDatasourceProvider`
  - [ ] `credentials_are_configured()` class method (credential validation)
  - [ ] `execute_query()` with proper type mapping (especially date/time types)
  - [ ] `dry_run()` with error location parsing
  - [ ] `get_table_info()` for agent schema lookup
  - [ ] `test_connection()`
  - [ ] `close()` with proper cleanup
  - [ ] Discovery method (`list_databases()` or `list_schemas()`)
- [ ] Create `datasources/<type>/indexer.py` implementing `BaseSQLCatalogIndexer`
  - [ ] `_get_provider_class()` returns provider class for credential validation
  - [ ] `_create_provider()` creates provider instance
  - [ ] Implement 3 additional abstract methods (containers, tables, columns)
  - [ ] Set `container_label` and `container_config_key`
- [ ] Add Pydantic validators in `routers/datasources.py`
  - [ ] `<Type>ConnectionConfig` - connection validation
  - [ ] `<Type>Credentials` - credential validation
  - [ ] Add both to `CONNECTION_CONFIG_VALIDATORS` dict
- [ ] Add dependencies to `pyproject.toml`
- [ ] Add database enum value if needed

**What you DON'T need to do (registry handles it):**
- ❌ Manually register in `PROVIDER_REGISTRY` (removed)
- ❌ Manually register in `INDEXER_REGISTRY` (automated)
- ❌ Add switch statements in routers (registry-based routing)
- ❌ Update catalog tree building logic (reads from metadata)
- ❌ Update catalog status endpoint (reads from metadata)

### Backend (Optional Features)
- [ ] **SSH tunnel support** (if database may be behind firewall)
  - [ ] Add SSH config fields to ConnectionConfig
  - [ ] Implement `_create_ssh_tunnel()`
  - [ ] Implement `generate_ssh_keypair()` utility
  - [ ] Add sshtunnel and cryptography dependencies
- [ ] **Shared credentials** (if workspace-level auth is common)
  - [ ] Add `shared_credentials`, `shared_username`, `shared_password` fields
  - [ ] Implement credential resolution logic in provider `__init__`
- [ ] **Query pagination** (recommended for production)
  - [ ] Add `offset` and `include_total` parameters to `execute_query()`
  - [ ] Implement COUNT query for total rows
  - [ ] Add pagination metadata to QueryResult
- [ ] **Connection management** (recommended for production)
  - [ ] Instance-level connection state (`_connection`, `_tunnel`)
  - [ ] Context manager support (`__enter__`, `__exit__`)
  - [ ] Connection reuse with state checking

### Frontend (Required)
- [ ] Update `DatasourceSettings.jsx` with type metadata
  - [ ] Add to `DATASOURCE_TYPES` constant
  - [ ] Connection config form in `renderConnectionForm()`
  - [ ] Credentials form in `renderCredentialsForm()`
  - [ ] Catalog configuration UI (e.g., schema selector)
- [ ] **Add to ChartML proxy datasources** `lib/chartml/plugins/genericProxyDataSource.js`
  - [ ] Add type to `PROXY_DATASOURCES` array
  - [ ] Add error enhancements to `ERROR_ENHANCEMENTS` dict (optional)
  - [ ] Export named datasource (e.g., `export const mysqlDataSource = createProxyDataSource('mysql')`)
- [ ] Add datasource icon to `DatasourceIcon.jsx`
  - [ ] Create icon component (Lucide-style: 24x24, 2px stroke)
  - [ ] Add to `DATASOURCE_TYPE_INFO`
  - [ ] Add case to switch statement
- [ ] Update `CatalogSection.jsx`
  - [ ] Add to `supportsDiscovery` array
  - [ ] Add to `supportsSchemaDiscovery` array
  - [ ] Add case in `getCatalogConfigInfo()` switch

### Documentation (CRITICAL - Agent Knowledge)
- [ ] **Update ChartML JSON Schema** `docs/chartml-spec/chartml_schema.json`
  - [ ] Add type to `provider` enum in source definition (around line 42)
  - [ ] Add type to `provider` enum in inline data definition (around line 454)
- [ ] **Regenerate minified schema**
  - [ ] Run: `cd docs/chartml-spec && cat chartml_schema.json | jq -c > chartml_schema.min.json`
- [ ] **Update ChartML Specification** `docs/chartml-spec/SPECIFICATION.md`
  - [ ] Add type to provider list in Source structure (around line 82)
  - [ ] Add type to provider list in Inline data structure (around line 360)

Without these updates, the AI agent will think your datasource type is not supported for ChartML dashboards!

**Registry metadata is automatically exposed via `/api/v1/datasources/types` endpoint - no manual documentation needed!**

### Testing
- [ ] Test connection via Settings UI
  - [ ] Test successful connection
  - [ ] Test connection errors (wrong host, credentials, etc.)
  - [ ] Test SSH tunnel if implemented
- [ ] Test query execution via SQL Editor
  - [ ] Test successful queries
  - [ ] Test syntax errors (verify line numbers appear)
  - [ ] Test pagination (if implemented)
- [ ] Test catalog indexing
  - [ ] Verify tables appear in search
  - [ ] Test schema filtering (if applicable)
- [ ] **Test ChartML dashboards** with the new datasource type
  - [ ] Create test dashboard with charts
  - [ ] Verify data loads correctly
  - [ ] Test error handling

## SQL Dialect Considerations

The agent writes SQL based on the datasource type. Key dialect differences to document:

| Feature | BigQuery | PostgreSQL | ClickHouse |
|---------|----------|------------|------------|
| String quotes | `'single'` or `"double"` | `'single'` | `'single'` |
| Identifier quotes | `` `backticks` `` | `"double"` | `` `backticks` `` |
| Date functions | `DATE_TRUNC(date, MONTH)` | `DATE_TRUNC('month', date)` | `toStartOfMonth(date)` |
| Array access | `arr[OFFSET(0)]` | `arr[1]` | `arr[1]` |

The agent is informed of the dialect via the `_dialect` field in datasource context.

## Security Considerations

1. **Credentials encryption**: All credentials stored in `UserDatasourceCredential` use `EncryptedJSON`
2. **Connection config encryption**: Sensitive fields (SSH keys, etc.) in `DatasourceConfig.connection_config` use `EncryptedJSON`
3. **SQL injection**: Provider implementations should use parameterized queries where possible
4. **Network security**: Consider SSH tunnel support for databases behind firewalls (see PostgreSQL provider)

## PostgreSQL: Reference Implementation

The PostgreSQL provider (`apps/backend/src/api/datasources/postgres/`) is the **gold standard** reference implementation demonstrating all advanced features and best practices:

### Features Implemented

1. **Complete Provider Interface** (provider.py)
   - Full `BaseDatasourceProvider` implementation
   - Granular date/time type mapping (date, time, timestamp, timestamptz)
   - Complete OID-to-type mapping with 40+ PostgreSQL types
   - Error location parsing with line numbers for SQL Editor

2. **SSH Tunnel Support** (provider.py:98-139, 441-473)
   - Ed25519 keypair generation
   - SSHTunnelForwarder integration
   - Automatic tunnel creation and cleanup
   - Encrypted private key storage

3. **Shared Credentials** (routers/datasources.py:126-137)
   - Workspace-level credentials option
   - Credential resolution logic
   - Admin-only configuration

4. **Connection Management** (provider.py)
   - Instance-level connection state
   - Connection reuse with state checking
   - Context manager support (`__enter__`, `__exit__`)
   - Robust cleanup with error handling

5. **Query Pagination** (provider.py:209-320)
   - LIMIT/OFFSET support
   - Total row count with separate COUNT query
   - Intelligent LIMIT handling (respects existing LIMIT in query)
   - Pagination metadata in results

6. **SQL Validation** (provider.py:402-438)
   - `dry_run()` with EXPLAIN
   - Error location parsing from `diag.statement_position`
   - Rollback on validation errors

7. **Catalog Indexing** (indexer.py)
   - `information_schema` discovery
   - Schema filtering (`catalog_schemas` configuration)
   - System schema exclusion (pg_catalog, information_schema)
   - Table and column metadata extraction

8. **Frontend Integration**
   - ChartML plugin (lib/chartml/plugins/postgresDataSource.js)
   - DatasourceIcon with Slonik elephant logo
   - Settings UI with SSH tunnel configuration
   - Catalog schema selector

### File Structure

```
apps/backend/src/api/datasources/postgres/
├── __init__.py              # Package exports
├── provider.py              # PostgresProvider (680 lines)
│   ├── Connection management
│   ├── SSH tunnel support
│   ├── Query execution with pagination
│   ├── SQL validation (dry_run)
│   └── Type mapping (40+ types)
└── indexer.py               # PostgresCatalogIndexer
    ├── Schema discovery
    ├── Table/column indexing
    └── information_schema queries
```

### Usage as Template

When implementing a new datasource:

1. **Start with structure**: Copy postgres/ folder structure
2. **Simplify as needed**: Remove SSH tunnel if not needed, remove shared credentials if not needed
3. **Adapt type mapping**: Replace OID map with your database's type system
4. **Customize error parsing**: Adapt `_parse_error_location()` for your database
5. **Update catalog queries**: Replace `information_schema` with your database's system catalog

### Key Learnings from PostgreSQL Implementation

- **Separation of concerns**: Connection config (workspace) vs credentials (user)
- **Optional features**: SSH tunnels and shared credentials are opt-in, not required
- **Error handling**: Always rollback on validation errors to clear transaction state
- **Cleanup discipline**: Use try/finally to ensure resources are always released
- **Context managers**: Enable Pythonic resource management patterns

## Universal Datasource Setup Flow - CRITICAL

**Every datasource MUST follow this setup flow. No exceptions.**

The user experience for connecting ANY datasource should be consistent:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  Step 1: MINIMAL CREDENTIALS                                                 │
│  Enter only what's needed to authenticate (host, port, user, password)      │
│                        ↓                                                     │
│  [Test & Discover] button                                                    │
│                        ↓                                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│  Step 2: DROPDOWNS APPEAR (after successful test)                           │
│  Select from discovered resources:                                           │
│  - Warehouse (Snowflake)                                                     │
│  - Default Database                                                          │
│  - Default Schema                                                            │
│                        ↓                                                     │
│  [Next] button                                                               │
│                        ↓                                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│  Step 3: CATALOG SELECTION (what to index)                                  │
│  Multi-select from discovered databases/schemas/projects                    │
│                        ↓                                                     │
│  [Create] button                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Step 1: Minimal Credentials (Per Datasource)

| Datasource | Required Fields | Notes |
|------------|----------------|-------|
| BigQuery | OAuth Connect button | No form fields - just OAuth |
| Snowflake | Account, Username, Password | Account ID from URL |
| PostgreSQL | Host, Port, Username, Password | Standard DB connection |
| ClickHouse | Host, Port, Username, Password | HTTP port (8123) |
| MySQL | Host, Port, Username, Password | Standard DB connection |
| Databricks | Server Hostname, HTTP Path, Token | From workspace settings |
| Redshift | Host, Port, Username, Password | Cluster endpoint |
| SQL Server | Host, Port, Username, Password | Include instance if needed |

**Key Principle:** Only ask for what's needed to CONNECT. Don't ask for database/schema/warehouse - those come from discovery.

### Step 2: Discovery → Dropdowns (Per Datasource)

After successful `Test & Discover`, the provider returns available resources. These populate **dropdown selects** (not text fields):

| Datasource | Discovery Method | Returns | Dropdowns Shown |
|------------|-----------------|---------|-----------------|
| BigQuery | OAuth scope | Projects | Billing Project, Default Project |
| Snowflake | `SHOW WAREHOUSES`, `SHOW DATABASES` | Warehouses, Databases | Warehouse, Default Database |
| PostgreSQL | `SELECT datname FROM pg_database`, schema query | Databases, Schemas | Default Database, Default Schema |
| ClickHouse | `SHOW DATABASES` | Databases | Default Database |
| MySQL | `SHOW DATABASES` | Databases | Default Database |
| Databricks | REST API | Catalogs, Schemas | Catalog, Schema |
| Redshift | Schema query | Schemas | Default Schema |
| SQL Server | `sys.databases`, `sys.schemas` | Databases, Schemas | Default Database, Default Schema |

### Step 3: Catalog Selection (Multi-Select)

Same resources from discovery, but as a multi-select checkbox list for "what to index":

| Datasource | Catalog Label | What's Indexed |
|------------|---------------|----------------|
| BigQuery | "Projects to Index" | Selected GCP projects |
| Snowflake | "Databases to Index" | Selected Snowflake databases |
| PostgreSQL | "Schemas to Index" | Selected PostgreSQL schemas |
| ClickHouse | "Databases to Index" | Selected ClickHouse databases |
| MySQL | "Databases to Index" | Selected MySQL databases |
| Databricks | "Catalogs to Index" | Selected Unity catalogs |
| Redshift | "Schemas to Index" | Selected Redshift schemas |
| SQL Server | "Schemas to Index" | Selected SQL Server schemas |

### Provider Implementation Requirements

Each provider MUST implement these discovery methods:

```python
class MyProvider(BaseDatasourceProvider):
    """Provider must implement discovery for the universal setup flow."""

    def discover_resources(self) -> DiscoveryResult:
        """
        Discover all resources available after authentication.
        Called after successful test_connection().

        Returns:
            DiscoveryResult with structure:
            {
                "warehouses": [...],     # Snowflake only
                "databases": [...],      # Most providers
                "schemas": [...],        # PostgreSQL, Redshift, SQL Server
                "projects": [...],       # BigQuery only
                "catalogs": [...],       # Databricks only
            }
        """
        # Implementation varies by provider
        pass

    def list_databases(self) -> DiscoveryResult:
        """List available databases (most providers)."""
        pass

    def list_schemas(self, database: str = None) -> DiscoveryResult:
        """List schemas in a database (PostgreSQL, Redshift, SQL Server)."""
        pass

    def list_warehouses(self) -> DiscoveryResult:
        """List warehouses (Snowflake only)."""
        pass
```

### Frontend Form Schema Updates

The `connectionFormSchemas.js` should distinguish between:

1. **Connection Fields** - Required for initial connect (always text inputs)
2. **Discovery Fields** - Populated after connect (become dropdowns)

```javascript
export const SNOWFLAKE_SCHEMA = {
  type: 'snowflake',
  // Step 1: Connection fields (text inputs, required for connect)
  connectionFields: [
    { name: 'account', type: 'text', required: true, helpText: 'Found in your Snowflake URL' },
  ],
  // Credentials (separate section)
  credentialFields: [
    { name: 'username', type: 'text', required: true },
    { name: 'password', type: 'password', required: true },
  ],
  // Step 2: Discovery fields (become dropdowns after connect)
  discoveryFields: [
    { name: 'warehouse', type: 'discovery', discoveryKey: 'warehouses', label: 'Warehouse' },
    { name: 'database', type: 'discovery', discoveryKey: 'databases', label: 'Default Database' },
    { name: 'schema', type: 'discovery', discoveryKey: 'schemas', label: 'Default Schema', optional: true },
  ],
  // Step 3: Catalog selection
  catalogConfig: {
    key: 'catalog_databases',
    label: 'Databases to Index',
    discoveryKey: 'databases',
  },
};
```

### API Endpoints for Discovery

```
POST /api/v1/datasources/test-connection
  Body: { datasource_type, connection_config, credentials }
  Returns: { success: bool, message: str }

POST /api/v1/datasources/discover
  Body: { datasource_type, connection_config, credentials }
  Returns: { success: bool, warehouses: [], databases: [], schemas: [], ... }

POST /api/v1/datasources/discover-catalog
  Body: { datasource_type, connection_config, credentials }
  Returns: { success: bool, items: [], item_type: str }
```

### Visual Flow Example (Snowflake)

```
┌─────────────────────────────────────────────┐
│ Add Snowflake Connection                     │
├─────────────────────────────────────────────┤
│                                             │
│ Account:  [UHJPTST-VU45595        ]         │
│           ↳ Found in your Snowflake URL     │
│                                             │
│ ─── Credentials ───                         │
│ Username: [alyticjason              ]       │
│ Password: [••••••••                 ]       │
│                                             │
│ [Test & Discover]  ✓ Connected              │
│                                             │
│ ─── Select Defaults ─── (appears after)     │
│ Warehouse: [▼ INTEGRATION_TEST_WH   ]       │
│ Database:  [▼ INTEGRATION_TESTS     ]       │
│ Schema:    [▼ TEST_DATA            ]        │
│                                             │
│                              [Next →]       │
└─────────────────────────────────────────────┘
                    ↓
┌─────────────────────────────────────────────┐
│ Select Databases to Index                    │
├─────────────────────────────────────────────┤
│                                             │
│ [Select All] [Clear]     2 of 4 selected    │
│                                             │
│ ┌─────────────────────────────────────────┐ │
│ │ ☑ INTEGRATION_TESTS                     │ │
│ │ ☑ PRODUCTION                            │ │
│ │ ☐ SNOWFLAKE_SAMPLE_DATA                 │ │
│ │ ☐ DEV_SANDBOX                           │ │
│ └─────────────────────────────────────────┘ │
│                                             │
│                             [Create]        │
└─────────────────────────────────────────────┘
```

---

## Frontend UX Requirements - SIMPLICITY FIRST

**Design Principle: One button. One modal. Tabs for organization.**

Read `docs/specifications/DATASOURCE_SETTINGS_UX_PLAN.md` for full details.

### UI Structure

1. **List View**: Clean list with [Settings] button per datasource
2. **Settings Modal**: Tabbed modal (Connection | Credentials | Catalog) for admins
3. **Warning Badge**: ⚠️ shown only when catalog needs attention

### Modal Tabs

| Tab | Who Sees It | Contents |
|-----|-------------|----------|
| Connection | Admin only | Host, port, database, SSL, etc. |
| Credentials | All users | OAuth (BigQuery) or username/password |
| Catalog | Admin only | Table count, last indexed, refresh, what to index |

Non-admins see only the Credentials form (no tabs).

### Consistent Terminology

| Datasource | Catalog Config Label |
|------------|---------------------|
| BigQuery | "Projects to Index" |
| PostgreSQL | "Schemas to Index" |
| ClickHouse | "Databases to Index" |
| MySQL | "Databases to Index" |
| Snowflake | "Databases to Index" |

### Backend Checklist for New Datasources

**With registry-based architecture, most of these are automatic!**

- [ ] Provider class created in `datasources/<type>/provider.py`
- [ ] Indexer class created in `datasources/<type>/indexer.py`
- [ ] **Registry metadata** in `datasources/<type>/__init__.py` with `DatasourceTypeRegistry.register()`
- [ ] **Pydantic schemas** in `schemas/datasource_schemas.py`:
  - [ ] `<Type>ConnectionConfig` added to `CONFIG_VALIDATORS`
  - [ ] `<Type>Credentials` model added
  - [ ] Added to `SUPPORTED_DATASOURCE_TYPES` list
- [ ] Provider implements discovery method (`list_schemas()` or `list_databases()`)
- [ ] Database enum value added (if needed): `ALTER TYPE datasource_type ADD VALUE '<type>';`

**What you DON'T need to do (registry handles automatically):**
- ❌ No switch statements in routers (registry-based routing)
- ❌ No manual PROVIDER_REGISTRY entries (removed)
- ❌ No manual indexer registration (automatic via registry)
- ❌ No catalog endpoint modifications (generic, reads from registry)

### Frontend Checklist for New Datasources

- [ ] Type added to `DATASOURCE_TYPES` in DatasourceSettings.jsx
- [ ] Connection form renders for new type in `renderConnectionForm()`
- [ ] Credentials form renders for new type in `renderCredentialsForm()`
- [ ] Icon added to `DatasourceIcon` component
- [ ] **CatalogSection.jsx updates:**
  - [ ] Add to `supportsDiscovery` array (~line 60)
  - [ ] Add to `supportsSchemaDiscovery` array (~line 89)
  - [ ] Add case in `getCatalogConfigInfo()` switch (~line 140)

## Known Gaps / TODO

**Status of multi-datasource implementation:**

### Completed

| Item | Description | Status |
|------|-------------|--------|
| **Generic Catalog Tree Endpoint** | `GET /api/v1/datasources/{id}/catalog/tree` | ✅ Done |
| **Generic Catalog Tree Component** | `DatasourceCatalogTree.jsx` replaces BigQuery-specific tree | ✅ Done |
| **Catalog Config (Backend)** | `catalog_schemas` in PostgresConnectionConfig | ✅ Done |
| **Catalog Config (Basic UI)** | Schema selector in DatasourceSettings.jsx | ✅ Done |
| **Multi-datasource Scheduler** | Catalog refresh scheduler handles all datasource types | ✅ Done |
| **SQL Dry Run** | Generic dry_run() with provider-specific line/column parsing | ✅ Done |
| **Catalog Status Endpoint** | `GET /api/v1/datasources/{id}/catalog/status` in `routers/catalog.py` | ✅ Done |
| **Manual Refresh Endpoint** | `POST /api/v1/datasources/{id}/catalog/refresh` in `routers/catalog.py` | ✅ Done |
| **Schema Discovery Endpoint** | `GET /api/v1/datasources/{id}/schemas` in `routers/catalog.py` | ✅ Done |
| **Registry-Based Architecture** | `DatasourceTypeRegistry` eliminates all switch statements | ✅ Done |
| **Catalog Router Extraction** | Catalog endpoints moved to `routers/catalog.py` | ✅ Done |

### Remaining Gaps

| Gap | Description | Priority |
|-----|-------------|----------|
| **Catalog Section Redesign** | Move catalog from modal to inline section per `DATASOURCE_SETTINGS_UX_PLAN.md` | MEDIUM |
| **Auto-save with Feedback** | Credentials auto-save on blur with "Saving..." indicator | LOW |
| **Blocking vs Non-blocking Indexing** | Clarify whether catalog indexing blocks datasource creation or runs async in background | LOW |

### Implementation Reference

See `docs/specifications/DATASOURCE_SETTINGS_UX_PLAN.md` for the full UX redesign plan including:
- Three-section layout (Connection, Credentials, Catalog)
- Catalog section requirements (inline, not modal)
- API endpoints needed
- Component architecture

### When You Encounter a Gap

If you're implementing a datasource and find something missing from this doc:
1. **Fix it immediately** - add the missing information
2. **Add to Known Gaps** if it's a broader architectural issue
3. Keep this document accurate so the next person doesn't hit the same issues
