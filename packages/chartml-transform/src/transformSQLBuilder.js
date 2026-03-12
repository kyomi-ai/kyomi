// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Aggregate SQL Builder (ChartML v1.0)
 *
 * Generates DuckDB SQL queries from ChartML v1.0 aggregate specifications.
 *
 * Aggregate spec structure:
 * {
 *   dimensions: [
 *     "product",
 *     { column: "DATE_TRUNC(sale_date, 'MONTH')", name: "month" }
 *   ],
 *   measures: [
 *     { column: "revenue", aggregation: "sum", name: "total_revenue" },
 *     { expression: "total_revenue / total_units", name: "avg_price" }
 *   ],
 *   filters: {
 *     combinator: "and",
 *     rules: [
 *       { field: "category", operator: "=", value: "Electronics" },
 *       { field: "total_revenue", operator: ">=", value: 50000 }
 *     ]
 *   },
 *   sort: [
 *     { field: "month", direction: "asc" }
 *   ],
 *   limit: 100
 * }
 *
 * Generates SQL:
 * SELECT
 *   product,
 *   DATE_TRUNC(sale_date, 'MONTH') as month,
 *   SUM(revenue) as total_revenue,
 *   SUM(units) as total_units,
 *   (SUM(revenue) / SUM(units)) as avg_price
 * FROM __extract_abc123
 * WHERE category = 'Electronics'
 * GROUP BY product, DATE_TRUNC(sale_date, 'MONTH')
 * HAVING SUM(revenue) >= 50000
 * ORDER BY month ASC
 * LIMIT 100
 */

/**
 * Supported aggregation functions (ChartML v1.0 camelCase)
 */
const AGGREGATION_FUNCTIONS = new Set([
  'sum',
  'avg',
  'count',
  'countdistinct',
  'min',
  'max',
  'median',
  'stddev',
  'variance',
  'percentile25',
  'percentile50',
  'percentile75',
  'percentile90',
  'percentile95',
  'percentile99'
]);

/**
 * Quote a SQL identifier (column name, table name, etc.)
 * Handles SQL keywords and special characters
 *
 * @param {string} identifier - The identifier to quote
 * @returns {string} Quoted identifier
 */
function quoteIdentifier(identifier) {
  // Guard against undefined/null
  if (!identifier || typeof identifier !== 'string') {
    return '""';
  }

  // If it's already a complex expression (contains parentheses for functions or * for wildcards), don't quote
  // Note: Column names with spaces still need quotes - they're not expressions
  if (identifier.includes('(') || identifier.includes('*')) {
    return identifier;
  }
  // Quote simple identifiers to handle SQL keywords and column names with spaces
  return `"${identifier}"`;
}

/**
 * Operator mapping from filter format to DuckDB SQL (ChartML v1.0 camelCase)
 */
const OPERATOR_MAP = {
  '=': '=',
  '!=': '!=',
  '<': '<',
  '>': '>',
  '<=': '<=',
  '>=': '>=',
  'contains': 'LIKE',
  'startsWith': 'LIKE',
  'endsWith': 'LIKE',
  'isNull': 'IS NULL',
  'isNotNull': 'IS NOT NULL',
  'in': 'IN',
  'notIn': 'NOT IN',
  'between': 'BETWEEN'
};

/**
 * Build a symbol table from dimensions and measures
 * Maps field names to their SQL expressions
 *
 * @param {Array} dimensions - Dimension definitions
 * @param {Array} measures - Measure definitions
 * @returns {Object} Symbol table mapping field names to SQL expressions
 */
function buildSymbolTable(dimensions = [], measures = []) {
  const symbols = {};

  // Process dimensions
  for (const dim of dimensions) {
    if (typeof dim === 'string') {
      // Shorthand: column name becomes field name
      symbols[dim] = {
        sql: quoteIdentifier(dim),
        type: 'dimension',
        isAggregated: false
      };
    } else {
      // Object form
      const fieldName = dim.name || dim.column;
      const sqlExpr = dim.column || dim.expression;

      symbols[fieldName] = {
        sql: sqlExpr,  // Don't quote here - could be an expression like DATE_TRUNC(...)
        type: 'dimension',
        isAggregated: false
      };
    }
  }

  // Process measures (needs two passes for calculated measures)
  const aggregatedMeasures = {};
  const calculatedMeasures = {};

  // First pass: aggregated measures
  for (const measure of measures) {
    if (measure.aggregation) {
      // Has aggregation - this is a direct aggregate
      const fieldName = measure.name;
      const column = measure.column;
      const agg = measure.aggregation.toLowerCase();

      if (!AGGREGATION_FUNCTIONS.has(agg)) {
        continue;
      }

      let sqlExpr;
      if (agg === 'countDistinct' || agg === 'countdistinct') {
        sqlExpr = `COUNT(DISTINCT ${quoteIdentifier(column)})`;
      } else if (agg.startsWith('percentile')) {
        // Handle percentile functions (percentile25, percentile50, etc.)
        const percentileValue = agg.replace('percentile', '');
        sqlExpr = `PERCENTILE_CONT(${quoteIdentifier(column)}, 0.${percentileValue})`;
      } else {
        sqlExpr = `${agg.toUpperCase()}(${quoteIdentifier(column)})`;
      }

      aggregatedMeasures[fieldName] = {
        sql: sqlExpr,
        type: 'measure',
        isAggregated: true,
        column: column,
        aggregation: agg
      };
    } else if (measure.expression) {
      // Post-aggregation calculation - process in second pass
      calculatedMeasures[measure.name] = measure.expression;
    }
  }

  // Add aggregated measures to symbol table
  Object.assign(symbols, aggregatedMeasures);

  // Second pass: calculated measures (resolve field references)
  // Process in order, building up the symbol table so later calculations can reference earlier ones
  for (const [fieldName, expression] of Object.entries(calculatedMeasures)) {
    // Replace field references with their SQL expressions
    // Use the full symbol table so calculated measures can reference each other
    const resolvedSQL = resolveExpression(expression, symbols);

    symbols[fieldName] = {
      sql: resolvedSQL,
      type: 'measure',
      isAggregated: true,
      expression: expression
    };
  }

  return symbols;
}

/**
 * Resolve field references in an expression to SQL
 *
 * @param {string} expression - Expression with field references
 * @param {Object} symbolTable - Symbol table with field definitions
 * @returns {string} SQL expression with resolved references
 */
function resolveExpression(expression, symbolTable) {
  let resolved = expression;

  // Sort field names by length (longest first) to avoid partial replacements
  const fieldNames = Object.keys(symbolTable).sort((a, b) => b.length - a.length);

  for (const fieldName of fieldNames) {
    const symbol = symbolTable[fieldName];
    // Replace field name with its SQL expression
    // Use word boundaries to avoid partial matches
    const regex = new RegExp(`\\b${escapeRegex(fieldName)}\\b`, 'g');
    resolved = resolved.replace(regex, symbol.sql);
  }

  return `(${resolved})`;
}

/**
 * Escape special regex characters in a string
 */
function escapeRegex(str) {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Format a filter value for SQL
 *
 * @param {*} value - The filter value
 * @param {string} operator - The operator being used
 * @returns {string} SQL-formatted value
 */
function formatFilterValue(value, operator) {
  // NULL operators don't need values
  if (operator === 'isNull' || operator === 'isNotNull') {
    return '';
  }

  // IN/NOT IN operators expect arrays
  if (operator === 'in' || operator === 'notIn') {
    if (!Array.isArray(value)) {
      value = [value];
    }
    return '(' + value.map(v =>
      typeof v === 'string' ? `'${v.replace(/'/g, "''")}'` : v
    ).join(', ') + ')';
  }

  // BETWEEN expects array of two values
  if (operator === 'between' || operator === 'not_between') {
    if (!Array.isArray(value) || value.length !== 2) {
      return '';
    }
    const v1 = typeof value[0] === 'string' ? `'${value[0].replace(/'/g, "''")}'` : value[0];
    const v2 = typeof value[1] === 'string' ? `'${value[1].replace(/'/g, "''")}'` : value[1];
    return `${v1} AND ${v2}`;
  }

  // LIKE operators need wildcards
  if (operator === 'contains') {
    return `'%${String(value).replace(/'/g, "''")}%'`;
  }
  if (operator === 'startsWith') {
    return `'${String(value).replace(/'/g, "''")}%'`;
  }
  if (operator === 'endsWith') {
    return `'%${String(value).replace(/'/g, "''")}'`;
  }

  // String values need quotes
  if (typeof value === 'string') {
    return `'${value.replace(/'/g, "''")}'`;
  }

  // Numbers and booleans as-is
  return value;
}

/**
 * Build a SQL condition from a single filter rule
 *
 * @param {Object} rule - Filter rule object
 * @param {Object} symbolTable - Symbol table for resolving field names
 * @returns {string} SQL condition
 */
function buildFilterCondition(rule, symbolTable) {
  const { field, operator, value } = rule;

  if (!field || !operator) {
    return '';
  }

  const sqlOperator = OPERATOR_MAP[operator];
  if (!sqlOperator) {
    return '';
  }

  // Handle empty arrays for IN/NOT IN operators
  if ((operator === 'in' || operator === 'not_in') && Array.isArray(value) && value.length === 0) {
    // For IN with empty array, nothing can match - return always false
    // For NOT IN with empty array, everything matches - return always true
    return operator === 'in' ? '(1=0)' : '(1=1)';
  }

  // Resolve field to SQL expression
  const symbol = symbolTable[field];
  if (!symbol) {
    // Field not in symbol table - assume it's a raw column
  }

  const sqlExpr = symbol ? symbol.sql : quoteIdentifier(field);

  // Build the condition
  const formattedValue = formatFilterValue(value, operator);

  if (operator === 'is_null' || operator === 'is_not_null') {
    return `${sqlExpr} ${sqlOperator}`;
  }

  return `${sqlExpr} ${sqlOperator} ${formattedValue}`;
}

/**
 * Build WHERE or HAVING clause from filter configuration
 *
 * @param {Object} filterConfig - Filter configuration with combinator and rules
 * @param {Object} symbolTable - Symbol table for resolving field names
 * @returns {string} SQL conditions (without WHERE/HAVING keyword)
 */
function buildFilterClause(filterConfig, symbolTable) {
  if (!filterConfig || !filterConfig.rules || filterConfig.rules.length === 0) {
    return '';
  }

  const { combinator = 'and', rules } = filterConfig;
  const conditions = [];

  for (const rule of rules) {
    // Check if this is a nested group (has its own combinator and rules)
    if (rule.combinator && rule.rules) {
      const nestedCondition = buildFilterClause(rule, symbolTable);
      if (nestedCondition) {
        conditions.push(`(${nestedCondition})`);
      }
    } else {
      // Simple rule
      const condition = buildFilterCondition(rule, symbolTable);
      if (condition) {
        conditions.push(condition);
      }
    }
  }

  if (conditions.length === 0) {
    return '';
  }

  const sqlCombinator = combinator.toUpperCase();
  return conditions.join(` ${sqlCombinator} `);
}

/**
 * Partition filters into WHERE (pre-aggregation) and HAVING (post-aggregation)
 *
 * @param {Object} filterConfig - Filter configuration
 * @param {Object} symbolTable - Symbol table
 * @returns {Object} { whereFilters, havingFilters }
 */
function partitionFilters(filterConfig, symbolTable) {
  if (!filterConfig || !filterConfig.rules || filterConfig.rules.length === 0) {
    return { whereFilters: null, havingFilters: null };
  }

  const whereRules = [];
  const havingRules = [];

  for (const rule of filterConfig.rules) {
    // Check if nested group
    if (rule.combinator && rule.rules) {
      // Recursively partition nested groups
      const nested = partitionFilters(rule, symbolTable);
      if (nested.whereFilters) whereRules.push(nested.whereFilters);
      if (nested.havingFilters) havingRules.push(nested.havingFilters);
    } else {
      // Check if field is a measure (aggregated)
      const symbol = symbolTable[rule.field];
      if (symbol && symbol.isAggregated) {
        havingRules.push(rule);
      } else {
        whereRules.push(rule);
      }
    }
  }

  const combinator = filterConfig.combinator || 'and';

  return {
    whereFilters: whereRules.length > 0 ? { combinator, rules: whereRules } : null,
    havingFilters: havingRules.length > 0 ? { combinator, rules: havingRules } : null
  };
}

/**
 * Build SQL query from aggregate specification (ChartML v1.0)
 *
 * @param {string} tableName - The DuckDB table name (e.g., __extract_<hash>)
 * @param {Object} aggregateSpec - Aggregate specification (optional - defaults to passthrough)
 * @param {Array} aggregateSpec.dimensions - Dimension definitions
 * @param {Array} aggregateSpec.measures - Measure definitions
 * @param {Object} aggregateSpec.filters - Filter configuration
 * @param {Array} aggregateSpec.sort - Sort configuration
 * @param {number} aggregateSpec.limit - Row limit
 * @returns {string} DuckDB SQL query
 */
export function buildAggregateSQL(tableName, aggregateSpec = {}) {
  const {
    dimensions = [],
    measures = [],
    filters = null,
    sort = [],
    limit = null,
    offset = null
  } = aggregateSpec;

  // Check if this is a passthrough (no dimensions, no measures)
  const isPassthrough = dimensions.length === 0 && measures.length === 0;

  if (isPassthrough) {
    // Simple SELECT * with optional filters, sort, limit
    let sql = `SELECT * FROM ${tableName}`;

    // Apply filters (can only be WHERE since no measures)
    if (filters) {
      const whereConditions = buildFilterClause(filters, {});
      if (whereConditions) {
        sql += `\nWHERE ${whereConditions}`;
      }
    }

    // Apply sort
    if (sort && sort.length > 0) {
      const orderClauses = sort.map(s => `${quoteIdentifier(s.field)} ${s.direction.toUpperCase()}`);
      sql += `\nORDER BY ${orderClauses.join(', ')}`;
    }

    // Apply limit and offset
    if (limit !== null && limit !== undefined) {
      sql += `\nLIMIT ${limit}`;
      if (offset !== null && offset !== undefined) {
        sql += ` OFFSET ${offset}`;
      }
    }

    return sql;
  }

  // Build symbol table
  const symbolTable = buildSymbolTable(dimensions, measures);

  // Determine if we need aggregation
  const hasAggregation = measures.some(m => m.aggregation || m.expression);

  // Build SELECT clause
  const selectCols = [];
  const groupByCols = [];

  // Add dimensions to SELECT and GROUP BY
  for (const dim of dimensions) {
    if (typeof dim === 'string') {
      const quotedDim = quoteIdentifier(dim);
      selectCols.push(quotedDim);
      if (hasAggregation) {
        groupByCols.push(quotedDim);
      }
    } else {
      const fieldName = dim.name || dim.column;
      const sqlExpr = dim.column || dim.expression;

      if (fieldName === sqlExpr) {
        selectCols.push(quoteIdentifier(fieldName));
      } else {
        selectCols.push(`${sqlExpr} as ${quoteIdentifier(fieldName)}`);
      }

      if (hasAggregation) {
        groupByCols.push(sqlExpr);
      }
    }
  }

  // Add measures to SELECT
  for (const measure of measures) {
    const fieldName = measure.name;
    const symbol = symbolTable[fieldName];

    if (!symbol) {
      continue;
    }

    if (fieldName === symbol.sql) {
      selectCols.push(quoteIdentifier(fieldName));
    } else {
      selectCols.push(`${symbol.sql} as ${quoteIdentifier(fieldName)}`);
    }
  }

  // Build WHERE and HAVING clauses
  let whereClause = '';
  let havingClause = '';

  if (filters) {
    const { whereFilters, havingFilters } = partitionFilters(filters, symbolTable);

    if (whereFilters) {
      const whereConditions = buildFilterClause(whereFilters, symbolTable);
      if (whereConditions) {
        whereClause = `\nWHERE ${whereConditions}`;
      }
    }

    if (havingFilters) {
      const havingConditions = buildFilterClause(havingFilters, symbolTable);
      if (havingConditions) {
        havingClause = `\nHAVING ${havingConditions}`;
      }
    }
  }

  // Build GROUP BY clause
  const groupByClause = hasAggregation && groupByCols.length > 0
    ? `\nGROUP BY ${groupByCols.join(', ')}`
    : '';

  // Build ORDER BY clause
  let orderByClause = '';
  if (sort && sort.length > 0) {
    const orderClauses = sort.map(s => {
      const field = s.field;
      const direction = s.direction.toUpperCase();
      // Use field name (alias) in ORDER BY - quote to handle keywords
      return `${quoteIdentifier(field)} ${direction}`;
    });
    orderByClause = `\nORDER BY ${orderClauses.join(', ')}`;
  }

  // Build LIMIT clause
  let limitClause = '';
  if (limit !== null && limit !== undefined) {
    limitClause = `\nLIMIT ${limit}`;
    if (offset !== null && offset !== undefined) {
      limitClause += ` OFFSET ${offset}`;
    }
  }

  // Construct final SQL
  const selectClause = selectCols.join(',\n  ');
  const sql = `SELECT\n  ${selectClause}\nFROM ${tableName}${whereClause}${groupByClause}${havingClause}${orderByClause}${limitClause}`;

  return sql;
}

/**
 * Check if transform spec requires aggregation
 *
 * @param {Object} transformSpec - Transform specification
 * @returns {boolean} True if aggregation is needed
 */
export function requiresAggregation(transformSpec) {
  const { measures = [] } = transformSpec;
  return measures.some(m => m.aggregation || m.expression);
}
