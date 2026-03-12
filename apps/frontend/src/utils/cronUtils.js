// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Cron utilities for displaying UTC cron expressions in local time
 *
 * Cron expressions are stored in UTC. This utility converts them to
 * human-readable descriptions in the user's local timezone.
 */

/**
 * Convert UTC hour to local hour (for display purposes)
 * Returns { localHour, dayOffset } where dayOffset is -1, 0, or +1
 */
export function utcToLocalHour(utcHour) {
  const now = new Date();
  const offsetMinutes = now.getTimezoneOffset();
  const offsetHours = offsetMinutes / 60;

  let localHour = utcHour - offsetHours;
  let dayOffset = 0;

  if (localHour >= 24) {
    localHour -= 24;
    dayOffset = 1;
  } else if (localHour < 0) {
    localHour += 24;
    dayOffset = -1;
  }

  return { localHour: Math.floor(localHour), dayOffset };
}

/**
 * Convert local hour to UTC hour (for building cron expressions)
 * Returns { utcHour, dayOffset } where dayOffset is -1, 0, or +1
 */
export function localHourToUtc(localHour) {
  const now = new Date();
  const offsetMinutes = now.getTimezoneOffset();
  const offsetHours = offsetMinutes / 60;

  let utcHour = localHour + offsetHours;
  let dayOffset = 0;

  if (utcHour >= 24) {
    utcHour -= 24;
    dayOffset = 1;
  } else if (utcHour < 0) {
    utcHour += 24;
    dayOffset = -1;
  }

  return { utcHour: Math.floor(utcHour), dayOffset };
}

/**
 * Parse a cron expression and return a human-readable description.
 * Handles standard 5-field cron: minute hour day-of-month month day-of-week
 * Note: Cron times are in UTC, but descriptions are shown in local time.
 *
 * @param {string} cronExpression - 5-field cron expression in UTC
 * @returns {{ valid: boolean, description: string }}
 */
export function describeCron(cronExpression) {
  if (!cronExpression || typeof cronExpression !== 'string') {
    return { valid: false, description: 'Invalid cron expression' };
  }

  const parts = cronExpression.trim().split(/\s+/);
  if (parts.length !== 5) {
    return { valid: false, description: 'Cron expression must have 5 fields: minute hour day month weekday' };
  }

  const [minute, hour, dayOfMonth, month, dayOfWeek] = parts;

  // Validate basic structure
  const isValidField = (field) => /^[\d,\-\*\/]+$/.test(field);
  if (!parts.every(isValidField)) {
    return { valid: false, description: 'Invalid characters in cron expression' };
  }

  try {
    const descriptions = [];

    // Time description - converts to local time for display
    const formatTime = (utcH, m) => {
      const { localHour } = utcToLocalHour(utcH);
      const hour12 = localHour === 0 ? 12 : localHour > 12 ? localHour - 12 : localHour;
      const ampm = localHour < 12 ? 'AM' : 'PM';
      const minStr = m.toString().padStart(2, '0');
      return `${hour12}:${minStr} ${ampm}`;
    };

    // Day of week names
    const weekdays = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

    // Parse weekday field and adjust for timezone day offset
    // dayOffset > 0 means local day is ahead of UTC (e.g., Sydney when it's yesterday in UTC)
    const parseWeekdays = (field, utcHour) => {
      if (field === '*') return null;
      const { dayOffset } = utcToLocalHour(utcHour);
      const days = [];
      const ranges = field.split(',');
      for (const range of ranges) {
        if (range.includes('-')) {
          const [start, end] = range.split('-').map(Number);
          for (let i = start; i <= end; i++) {
            // Adjust weekday index for timezone crossing
            // UTC day + dayOffset = local day
            const adjustedDay = (i + dayOffset + 7) % 7;
            days.push(weekdays[adjustedDay]);
          }
        } else {
          const dayNum = Number(range);
          const adjustedDay = (dayNum + dayOffset + 7) % 7;
          days.push(weekdays[adjustedDay]);
        }
      }
      return days;
    };

    // Determine schedule type and build description
    if (minute !== '*' && hour === '*' && dayOfMonth === '*' && month === '*' && dayOfWeek === '*') {
      // Every hour at specific minute
      descriptions.push(`Every hour at minute ${minute}`);
    } else if (minute !== '*' && hour.match(/^\*\/\d+$/) && dayOfMonth === '*' && month === '*' && dayOfWeek === '*') {
      // Step syntax for hours (e.g., */2 = every 2 hours)
      const step = parseInt(hour.split('/')[1]);
      const m = parseInt(minute);
      const minStr = m.toString().padStart(2, '0');
      descriptions.push(`Every ${step} hours at :${minStr}`);
    } else if (minute !== '*' && hour !== '*' && hour.includes(',') && dayOfMonth === '*' && month === '*' && dayOfWeek === '*') {
      // Specific hours (hourly with selected hours)
      const m = parseInt(minute);
      const hours = hour.split(',').map(h => parseInt(h.trim()));
      const times = hours.map(h => formatTime(h, m));
      if (times.length <= 3) {
        descriptions.push(`Daily at ${times.join(', ')}`);
      } else {
        descriptions.push(`Daily at ${times.length} times: ${times[0]}, ${times[1]}, ... ${times[times.length - 1]}`);
      }
    } else if (minute !== '*' && hour !== '*' && dayOfMonth === '*' && month === '*' && dayOfWeek === '*') {
      // Daily at specific time
      const h = parseInt(hour);
      const m = parseInt(minute);
      descriptions.push(`Daily at ${formatTime(h, m)}`);
    } else if (minute !== '*' && hour !== '*' && dayOfMonth === '*' && month === '*' && dayOfWeek !== '*') {
      // Weekly on specific days
      const h = parseInt(hour);
      const m = parseInt(minute);
      const days = parseWeekdays(dayOfWeek, h);
      if (days && days.length > 0) {
        if (days.length === 5 && !days.includes('Saturday') && !days.includes('Sunday')) {
          descriptions.push(`Weekdays at ${formatTime(h, m)}`);
        } else if (days.length === 2 && days.includes('Saturday') && days.includes('Sunday')) {
          descriptions.push(`Weekends at ${formatTime(h, m)}`);
        } else {
          descriptions.push(`${days.join(', ')} at ${formatTime(h, m)}`);
        }
      }
    } else if (minute !== '*' && hour !== '*' && dayOfMonth !== '*' && month === '*' && dayOfWeek === '*') {
      // Monthly on specific day
      const h = parseInt(hour);
      const m = parseInt(minute);
      const { dayOffset } = utcToLocalHour(h);
      let day = parseInt(dayOfMonth) - dayOffset;
      if (day < 1) day = 1;
      if (day > 28) day = 28;
      const suffix = day === 1 ? 'st' : day === 2 ? 'nd' : day === 3 ? 'rd' : 'th';
      descriptions.push(`Monthly on the ${day}${suffix} at ${formatTime(h, m)}`);
    } else {
      // Complex expression - show raw breakdown
      const timeDesc = hour === '*' ? 'every hour' : `at hour ${hour} UTC`;
      const minDesc = minute === '*' ? 'every minute' : `minute ${minute}`;
      const dayDesc = dayOfMonth === '*' ? 'every day' : `day ${dayOfMonth}`;
      const monthDesc = month === '*' ? '' : `in month ${month}`;
      const weekDesc = dayOfWeek === '*' ? '' : `on weekday ${dayOfWeek}`;

      descriptions.push(`Runs ${minDesc}, ${timeDesc}, ${dayDesc} ${monthDesc} ${weekDesc}`.trim());
    }

    return { valid: true, description: descriptions.join('; ') };
  } catch (e) {
    return { valid: false, description: 'Could not parse cron expression' };
  }
}

export default describeCron;
