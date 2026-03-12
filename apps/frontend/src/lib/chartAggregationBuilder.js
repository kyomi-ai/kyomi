// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ChartML Aggregation SQL Builder
 *
 * Generates DuckDB SQL queries from ChartML configuration to aggregate
 * data from cached extract tables.
 *
 * Example ChartML config:
 * {
 *   type: "line",
 *   x: "week",
 *   groupBy: "country_name",
 *   series: [
 *     { column: "score", aggregate: "avg", name: "Avg Score" },
 *     { column: "score", aggregate: "max", name: "Max Score" },
 *     { column: "rank", name: "Rank" } // no aggregate = include in GROUP BY
 *   ]
 * }
 *
 * Generates:
 * SELECT
 *   week,
 *   country_name,
 *   AVG(score) as "Avg Score",
 *   MAX(score) as "Max Score",
 *   rank as "Rank"
 * FROM __extract_<hash>
 * GROUP BY week, country_name, rank
 * ORDER BY week, country_name
 */

/**
 * Supported aggregate functions
 */
const AGGREGATE_FUNCTIONS = new Set([
  'sum',
  'avg',
  'count',
  'count_distinct',
  'min',
  'max'
]);

/**
 * Operator mapping from react-querybuilder format to DuckDB SQL
 */
const OPERATOR_MAP = {
  '=': '=',
  '!=': '!=',
  '<': '<',
  '>': '>',
  '<=': '<=',
  '>=': '>=',
  'contains': 'LIKE',
  'beginsWith': 'LIKE',
  'endsWith': 'LIKE',
  'null': 'IS NULL',
  'notNull': 'IS NOT NULL',
  'in': 'IN',
  'notIn': 'NOT IN',
  'between': 'BETWEEN',
  'notBetween': 'NOT BETWEEN'
};

/**
 * Convert a filter value to SQL format
 * @param {*} value - The filter value
 * @param {string} operator - The operator being used
 * @returns {string} SQL-formatted value
 */
function formatFilterValue(value, operator) {
  // NULL operators don't need values
  if (operator === 'null' || operator === 'notNull') {
    return '';
  }

  // IN/NOT IN operators expect arrays
  if (operator === 'in' || operator === 'notIn') {
    if (!Array.isArray(value)) {
      value = [value];
    }
    return '(' + value.map(v => typeof v === 'string' ? `'${v.replace(/'/g, "''")}'` : v).join(', ') + ')';
  }

  // BETWEEN expects array of two values
  if (operator === 'between' || operator === 'notBetween') {
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
  if (operator === 'beginsWith') {
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
 * @param {Object} rule - Filter rule object
 * @param {boolean} isHaving - Whether this is for HAVING clause (needs aggregate function)
 * @returns {string} SQL condition
 */
function buildFilterCondition(rule, isHaving = false) {
  const { field, operator, value, aggregate } = rule;

  if (!field || !operator) {
    return '';
  }

  const sqlOperator = OPERATOR_MAP[operator];
  if (!sqlOperator) {
    return '';
  }

  // For HAVING clause, wrap field in aggregate function
  let column = field;
  if (isHaving && aggregate) {
    const aggregateUpper = aggregate.toUpperCase();
    if (aggregate.toLowerCase() === 'count_distinct') {
      column = `COUNT(DISTINCT ${field})`;
    } else {
      column = `${aggregateUpper}(${field})`;
    }
  }

  // Build the condition
  const formattedValue = formatFilterValue(value, operator);

  if (operator === 'null' || operator === 'notNull') {
    return `${column} ${sqlOperator}`;
  }

  return `${column} ${sqlOperator} ${formattedValue}`;
}

/**
 * Build WHERE or HAVING clause from react-querybuilder filter object
 * @param {Object} filterConfig - Filter configuration with combinator and rules
 * @param {boolean} isHaving - Whether this is for HAVING clause
 * @returns {string} SQL WHERE/HAVING clause (without WHERE/HAVING keyword)
 */
function buildFilterClause(filterConfig, isHaving = false) {
  if (!filterConfig || !filterConfig.rules || filterConfig.rules.length === 0) {
    return '';
  }

  const { combinator = 'and', rules } = filterConfig;
  const conditions = [];

  for (const rule of rules) {
    // Check if this is a nested group (has its own combinator and rules)
    if (rule.combinator && rule.rules) {
      const nestedCondition = buildFilterClause(rule, isHaving);
      if (nestedCondition) {
        conditions.push(`(${nestedCondition})`);
      }
    } else {
      // Simple rule
      const condition = buildFilterCondition(rule, isHaving);
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
 * Build aggregation SQL from ChartML configuration
 *
 * @param {string} tableName - The DuckDB table name (e.g., __extract_<hash>)
 * @param {Object} chartConfig - ChartML configuration
 * @param {string} chartConfig.x - X-axis column for grouping
 * @param {string|Array<string>} chartConfig.groupBy - Additional grouping column(s)
 * @param {Array<Object>} chartConfig.series - Series definitions
 * @param {Object} chartConfig.where - Pre-aggregation filters (react-querybuilder format)
 * @param {Object} chartConfig.having - Post-aggregation filters (react-querybuilder format)
 * @param {Object} options - Query options
 * @param {number} options.limit - Optional row limit
 * @param {number} options.offset - Optional offset for pagination
 * @param {Array<{column: string, direction: string}>} options.orderBy - Optional custom sort order
 * @returns {string} DuckDB SQL query
 */
export function buildAggregationSQL(tableName, chartConfig, options = {}) {
  const { x, groupBy, series } = chartConfig;
  const { limit, offset, orderBy } = options;

  // Check if any series item has an aggregate
  const hasAggregates = series && series.some(s => s.aggregate);

  // If no aggregates, return simple SELECT * with optional WHERE filter
  if (!hasAggregates) {
    let sql = `SELECT * FROM ${tableName}`;

    // Apply WHERE filter even without aggregates
    if (chartConfig.where) {
      const whereConditions = buildFilterClause(chartConfig.where, false);
      if (whereConditions) {
        sql += `\nWHERE ${whereConditions}`;
      }
    }

    // Add ORDER BY if specified
    if (orderBy && orderBy.length > 0) {
      const orderClauses = orderBy.map(sort => `"${sort.column}" ${sort.direction.toUpperCase()}`);
      sql += `\nORDER BY ${orderClauses.join(', ')}`;
    }

    if (limit !== undefined) {
      sql += `\nLIMIT ${limit}`;
      if (offset !== undefined) {
        sql += ` OFFSET ${offset}`;
      }
    }
    return sql;
  }

  // Collect all columns for SELECT and GROUP BY
  const selectColumns = [];
  const groupByColumns = [];

  // Add x column (always a GROUP BY column if present)
  if (x) {
    selectColumns.push(x);
    groupByColumns.push(x);
  }

  // Add groupBy column(s) (always GROUP BY columns)
  if (groupBy) {
    const groupByArray = Array.isArray(groupBy) ? groupBy : [groupBy];
    for (const col of groupByArray) {
      selectColumns.push(col);
      groupByColumns.push(col);
    }
  }

  // Process series columns
  if (series && series.length > 0) {
    for (const seriesItem of series) {
      const { y, aggregate, name } = seriesItem;

      if (!y) {
        continue;
      }

      if (aggregate) {
        // Has aggregate function - build aggregate expression
        const aggregateUpper = aggregate.toUpperCase();

        if (!AGGREGATE_FUNCTIONS.has(aggregate.toLowerCase())) {
          continue;
        }

        let aggregateExpr;
        if (aggregate.toLowerCase() === 'count_distinct') {
          aggregateExpr = `COUNT(DISTINCT ${y})`;
        } else {
          aggregateExpr = `${aggregateUpper}(${y})`;
        }

        // Keep the original column name (y) as the alias
        // Note: 'name' is for legend display only, not SQL aliasing
        selectColumns.push(`${aggregateExpr} as ${y}`);

      } else {
        // No aggregate - this is a non-aggregate column, add to GROUP BY
        const alias = name || y;
        if (alias !== y) {
          selectColumns.push(`${y} as "${alias}"`);
        } else {
          selectColumns.push(y);
        }

        // Add to GROUP BY
        groupByColumns.push(y);
      }
    }
  }

  // Build SELECT clause
  const selectClause = selectColumns.join(',\n  ');

  // Build GROUP BY clause (always present when there are aggregates)
  const groupByClause = groupByColumns.length > 0
    ? `\nGROUP BY ${groupByColumns.join(', ')}`
    : '';

  // Build WHERE clause (pre-aggregation filters)
  let whereClause = '';
  if (chartConfig.where) {
    const whereConditions = buildFilterClause(chartConfig.where, false);
    if (whereConditions) {
      whereClause = `\nWHERE ${whereConditions}`;
    }
  }

  // Build HAVING clause (post-aggregation filters)
  let havingClause = '';
  if (chartConfig.having && hasAggregates) {
    const havingConditions = buildFilterClause(chartConfig.having, true);
    if (havingConditions) {
      havingClause = `\nHAVING ${havingConditions}`;
    }
  }

  // Build ORDER BY clause
  let orderByClause = '';
  if (orderBy && orderBy.length > 0) {
    // Use custom sort order from user
    const orderClauses = orderBy.map(sort => `${sort.column} ${sort.direction.toUpperCase()}`);
    orderByClause = `\nORDER BY ${orderClauses.join(', ')}`;
  } else if (groupByColumns.length > 0) {
    // Default: use GROUP BY columns for consistent ordering
    orderByClause = `\nORDER BY ${groupByColumns.join(', ')}`;
  }

  // Build LIMIT/OFFSET clause
  let limitClause = '';
  if (limit !== undefined) {
    limitClause = `\nLIMIT ${limit}`;
    if (offset !== undefined) {
      limitClause += ` OFFSET ${offset}`;
    }
  }

  // Construct final SQL
  const sql = `SELECT\n  ${selectClause}\nFROM ${tableName}${whereClause}${groupByClause}${havingClause}${orderByClause}${limitClause}`;

  return sql;
}

/**
 * Check if ChartML config requires aggregation
 *
 * @param {Object} chartConfig - ChartML configuration
 * @returns {boolean} True if aggregation is needed
 */
export function requiresAggregation(chartConfig) {
  const { series } = chartConfig;
  return series && series.some(s => s.aggregate);
}
