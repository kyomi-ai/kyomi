// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useMemo, useRef } from 'react';
import { Label } from '../ui/label';
import { Input } from '../ui/input';
import { Button } from '../ui/button';
import { Badge } from '../ui/badge';
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '../ui/select';
import { Switch } from '../ui/switch';
import { Clock, Code, AlertCircle } from 'lucide-react';
import { describeCron, utcToLocalHour, localHourToUtc } from '../../utils/cronUtils';

/**
 * Adjust weekdays array for day offset (when timezone conversion crosses midnight)
 */
function adjustWeekdays(weekdays, dayOffset) {
  if (dayOffset === 0 || weekdays.length === 0) return weekdays;

  return weekdays.map(day => {
    const adjusted = (parseInt(day) + dayOffset + 7) % 7;
    return adjusted.toString();
  });
}

/**
 * Adjust day of month for day offset
 */
function adjustDayOfMonth(day, dayOffset) {
  if (dayOffset === 0) return day;
  // For simplicity, just adjust by offset (edge cases at month boundaries are acceptable)
  const adjusted = day + dayOffset;
  if (adjusted < 1) return 1;
  if (adjusted > 31) return 31; // Max days in a month
  return adjusted;
}

/**
 * Build a cron expression from UI selections
 * Converts local time to UTC for the cron expression
 */
function buildCron({ type, minute, hour, weekdays, dayOfMonth, selectedHours = [] }) {
  const min = minute.toString();

  switch (type) {
    case 'hourly':
      // For hourly, selectedHours contains specific hours to run (e.g., [9, 12, 15, 18])
      if (selectedHours.length === 0) {
        return `${min} * * * *`; // Every hour if none selected
      }
      // Convert each local hour to UTC
      const utcHours = selectedHours.map(localHour => {
        const { utcHour } = localHourToUtc(localHour);
        return utcHour;
      }).sort((a, b) => a - b);
      return `${min} ${utcHours.join(',')} * * *`;
    case 'daily':
      const { utcHour: dailyUtcHour } = localHourToUtc(hour);
      return `${min} ${dailyUtcHour} * * *`;
    case 'weekly':
      const { utcHour: weeklyUtcHour, dayOffset: weeklyDayOffset } = localHourToUtc(hour);
      const adjustedDays = adjustWeekdays(weekdays, weeklyDayOffset);
      const days = adjustedDays.length > 0 ? adjustedDays.join(',') : '1';
      return `${min} ${weeklyUtcHour} * * ${days}`;
    case 'monthly':
      const { utcHour: monthlyUtcHour, dayOffset: monthlyDayOffset } = localHourToUtc(hour);
      const adjustedDay = adjustDayOfMonth(dayOfMonth, monthlyDayOffset);
      return `${min} ${monthlyUtcHour} ${adjustedDay} * *`;
    default:
      throw new Error(`Unknown schedule type: ${type}`);
  }
}

/**
 * Parse a cron expression back to UI selections
 * Converts UTC times in cron to local time for display
 */
function parseCronToSelections(cron) {
  const parts = cron.trim().split(/\s+/);
  if (parts.length !== 5) {
    return null;
  }

  const [minute, hour, dayOfMonth, month, dayOfWeek] = parts;

  // Parse minute - if invalid, can't use UI mode
  const parsedMinute = parseInt(minute);
  if (isNaN(parsedMinute)) return null;

  // Determine type based on pattern
  if (hour === '*' && dayOfMonth === '*' && dayOfWeek === '*') {
    // Every hour (old format)
    return { type: 'hourly', minute: parsedMinute, hour: 0, weekdays: [], dayOfMonth: 1, selectedHours: [] };
  }

  // Check for hourly patterns (e.g., "30 9,12,15 * * *" or "0 */2 * * *")
  if (hour !== '*' && dayOfMonth === '*' && dayOfWeek === '*') {
    // Check for step syntax (e.g., */2 means every 2 hours)
    const stepMatch = hour.match(/^\*\/(\d+)$/);
    if (stepMatch) {
      const step = parseInt(stepMatch[1]);
      if (step > 0 && step <= 12) {
        // Generate hours based on step (0, step, 2*step, etc.)
        const utcHours = [];
        for (let h = 0; h < 24; h += step) {
          utcHours.push(h);
        }
        // Convert UTC hours to local hours
        const localHours = utcHours.map(utcHour => {
          const { localHour } = utcToLocalHour(utcHour);
          return localHour;
        }).sort((a, b) => a - b);
        return { type: 'hourly', minute: parsedMinute, hour: 0, weekdays: [], dayOfMonth: 1, selectedHours: localHours };
      }
    }

    // Try to parse as comma-separated hours
    const hourParts = hour.split(',').map(h => parseInt(h.trim())).filter(h => !isNaN(h));
    if (hourParts.length > 0) {
      // Convert UTC hours to local hours
      const localHours = hourParts.map(utcHour => {
        const { localHour } = utcToLocalHour(utcHour);
        return localHour;
      }).sort((a, b) => a - b);
      return { type: 'hourly', minute: parsedMinute, hour: 0, weekdays: [], dayOfMonth: 1, selectedHours: localHours };
    }
  }

  // Parse hour - if invalid, can't use UI mode
  const parsedHour = parseInt(hour);
  if (isNaN(parsedHour)) return null;

  const { localHour, dayOffset } = utcToLocalHour(parsedHour);

  if (dayOfMonth === '*' && dayOfWeek === '*') {
    return { type: 'daily', minute: parsedMinute, hour: localHour, weekdays: [], dayOfMonth: 1 };
  }
  if (dayOfMonth === '*' && dayOfWeek !== '*') {
    // Adjust weekdays for day offset
    // UTC day + dayOffset = local day
    let weekdaysList = dayOfWeek.split(',').map(d => parseInt(d)).filter(d => !isNaN(d));
    if (weekdaysList.length === 0) return null; // Invalid weekday spec
    if (dayOffset !== 0) {
      weekdaysList = weekdaysList.map(d => (d + dayOffset + 7) % 7);
    }
    return { type: 'weekly', minute: parsedMinute, hour: localHour, weekdays: weekdaysList, dayOfMonth: 1 };
  }
  if (dayOfMonth !== '*' && dayOfWeek === '*') {
    // Parse day of month - if invalid, can't use UI mode
    const parsedDom = parseInt(dayOfMonth);
    if (isNaN(parsedDom)) return null;
    let dom = parsedDom;
    if (dayOffset !== 0) {
      dom = dom - dayOffset;
      if (dom < 1) dom = 1;
      if (dom > 31) dom = 31;
    }
    return { type: 'monthly', minute: parsedMinute, hour: localHour, weekdays: [], dayOfMonth: dom };
  }

  return null; // Complex expression, can't map to simple UI
}

/**
 * ScheduleSelector - Comprehensive schedule picker for watches
 *
 * Features:
 * - Schedule type: hourly, daily, weekly, monthly
 * - Time picker: hour and minute
 * - Day selector: weekdays for weekly, day of month for monthly
 * - Cron mode: raw cron input with human-readable description
 */
export default function ScheduleSelector({ value, onChange }) {
  // Parse current value
  const parsed = parseCronToSelections(value);
  const canUseUIMode = parsed !== null;

  const [cronMode, setCronMode] = useState(!canUseUIMode);
  const [cronInput, setCronInput] = useState(value);

  // UI mode state
  const [scheduleType, setScheduleType] = useState(parsed?.type ?? 'daily');
  const [minute, setMinute] = useState(parsed?.minute ?? 0);
  const [hour, setHour] = useState(parsed?.hour ?? 9);
  const [weekdays, setWeekdays] = useState(parsed?.weekdays ?? [1]);
  const [dayOfMonth, setDayOfMonth] = useState(parsed?.dayOfMonth ?? 1);
  const [selectedHours, setSelectedHours] = useState(parsed?.selectedHours ?? []);
  const [showHourSelection, setShowHourSelection] = useState((parsed?.selectedHours ?? []).length > 0);

  // Track the last value prop to detect external changes
  const lastValueRef = useRef(value);

  // Sync ALL state when value prop changes from parent
  useEffect(() => {
    if (value !== lastValueRef.current) {
      lastValueRef.current = value;
      setCronInput(value);
      const newParsed = parseCronToSelections(value);
      if (newParsed) {
        setScheduleType(newParsed.type);
        setMinute(newParsed.minute);
        setHour(newParsed.hour);
        setWeekdays(newParsed.weekdays);
        setDayOfMonth(newParsed.dayOfMonth);
        setSelectedHours(newParsed.selectedHours ?? []);
      }
    }
  }, [value]);

  // Track if user has made changes - only call onChange after user interaction
  const userHasInteracted = useRef(false);

  // Update cron when UI selections change (only after user interaction)
  useEffect(() => {
    if (!userHasInteracted.current) {
      return;
    }
    if (!cronMode) {
      const newCron = buildCron({ type: scheduleType, minute, hour, weekdays, dayOfMonth, selectedHours });
      onChange(newCron);
    }
  }, [cronMode, scheduleType, minute, hour, weekdays, dayOfMonth, selectedHours, onChange]);

  // Update parent when cron input changes in cron mode (only after user interaction)
  useEffect(() => {
    if (!userHasInteracted.current) {
      return;
    }
    if (cronMode) {
      onChange(cronInput);
    }
  }, [cronMode, cronInput, onChange]);

  // Sync UI state when switching from cron mode
  const handleModeSwitch = (useCron) => {
    userHasInteracted.current = true;
    if (!useCron) {
      // Switching to UI mode - try to parse current cron
      const newParsed = parseCronToSelections(cronInput);
      if (newParsed) {
        setScheduleType(newParsed.type);
        setMinute(newParsed.minute);
        setHour(newParsed.hour);
        setWeekdays(newParsed.weekdays);
        setDayOfMonth(newParsed.dayOfMonth);
        setSelectedHours(newParsed.selectedHours ?? []);
        setCronMode(false);
      }
      // If parsing fails, stay in cron mode (don't switch)
    } else {
      // Switching to cron mode - set cron from current UI
      const newCron = buildCron({ type: scheduleType, minute, hour, weekdays, dayOfMonth, selectedHours });
      setCronInput(newCron);
      setCronMode(true);
    }
  };

  // Check if current cron can be switched to UI mode
  const canSwitchToUIMode = useMemo(() => {
    return parseCronToSelections(cronInput) !== null;
  }, [cronInput]);

  // Wrap state setters to track user interaction
  const handleScheduleTypeChange = (type) => {
    userHasInteracted.current = true;
    setScheduleType(type);
    // Clear selectedHours when switching away from hourly
    if (type !== 'hourly' && selectedHours.length > 0) {
      setSelectedHours([]);
      setShowHourSelection(false);
    }
  };

  const handleHourChange = (v) => {
    userHasInteracted.current = true;
    setHour(parseInt(v));
  };

  const handleMinuteChange = (v) => {
    userHasInteracted.current = true;
    setMinute(parseInt(v));
  };

  const handleDayOfMonthChange = (v) => {
    userHasInteracted.current = true;
    setDayOfMonth(parseInt(v));
  };

  const handleCronInputChange = (e) => {
    userHasInteracted.current = true;
    setCronInput(e.target.value);
  };

  // Toggle weekday selection
  const toggleWeekday = (day) => {
    userHasInteracted.current = true;
    if (weekdays.includes(day)) {
      setWeekdays(weekdays.filter(d => d !== day));
    } else {
      setWeekdays([...weekdays, day].sort());
    }
  };

  // Toggle hour selection (for hourly mode)
  const toggleHour = (hour) => {
    userHasInteracted.current = true;
    if (selectedHours.includes(hour)) {
      setSelectedHours(selectedHours.filter(h => h !== hour));
    } else {
      setSelectedHours([...selectedHours, hour].sort((a, b) => a - b));
    }
  };

  // Generate hour options (for hourly mode checkboxes)
  const hourOptions = Array.from({ length: 24 }, (_, i) => {
    const hour12 = i === 0 ? 12 : i > 12 ? i - 12 : i;
    const ampm = i < 12 ? 'AM' : 'PM';
    return { value: i, label: `${hour12} ${ampm}` };
  });

  // Generate minute options - 5 minute intervals
  const minuteOptions = [0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55].map(m => ({
    value: m,
    label: m.toString().padStart(2, '0')
  }));

  // Generate 12-hour format hour options (1-12)
  const hour12Options = Array.from({ length: 12 }, (_, i) => ({
    value: i + 1,
    label: (i + 1).toString()
  }));

  // Get 12-hour format and AM/PM from 24-hour
  const getHour12 = (hour24) => {
    if (hour24 === 0) return 12;
    if (hour24 > 12) return hour24 - 12;
    return hour24;
  };

  const getAmPm = (hour24) => hour24 < 12 ? 'AM' : 'PM';

  // Convert 12-hour + AM/PM to 24-hour
  const to24Hour = (hour12, ampm) => {
    if (hour12 === 12) {
      return ampm === 'AM' ? 0 : 12;
    }
    return ampm === 'AM' ? hour12 : hour12 + 12;
  };

  // Handle 12-hour time component changes
  const handleHour12Change = (value) => {
    userHasInteracted.current = true;
    const hour12 = parseInt(value);
    const ampm = getAmPm(hour);
    setHour(to24Hour(hour12, ampm));
  };

  const handleAmPmChange = (value) => {
    userHasInteracted.current = true;
    const hour12 = getHour12(hour);
    setHour(to24Hour(hour12, value));
  };

  // Weekday options
  const weekdayOptions = [
    { value: 0, label: 'Sun', fullLabel: 'Sunday' },
    { value: 1, label: 'Mon', fullLabel: 'Monday' },
    { value: 2, label: 'Tue', fullLabel: 'Tuesday' },
    { value: 3, label: 'Wed', fullLabel: 'Wednesday' },
    { value: 4, label: 'Thu', fullLabel: 'Thursday' },
    { value: 5, label: 'Fri', fullLabel: 'Friday' },
    { value: 6, label: 'Sat', fullLabel: 'Saturday' },
  ];

  // Get ordinal suffix for a day number
  const getOrdinalSuffix = (day) => {
    if (day >= 11 && day <= 13) return 'th';
    const lastDigit = day % 10;
    if (lastDigit === 1) return 'st';
    if (lastDigit === 2) return 'nd';
    if (lastDigit === 3) return 'rd';
    return 'th';
  };

  // Day of month options (1-31)
  const dayOfMonthOptions = Array.from({ length: 31 }, (_, i) => {
    const day = i + 1;
    return {
      value: day,
      label: `${day}${getOrdinalSuffix(day)}`
    };
  });

  // Get human-readable description
  const cronDescription = useMemo(() => {
    const cron = cronMode ? cronInput : buildCron({ type: scheduleType, minute, hour, weekdays, dayOfMonth, selectedHours });
    return describeCron(cron);
  }, [cronMode, cronInput, scheduleType, minute, hour, weekdays, dayOfMonth, selectedHours]);

  return (
    <div className="space-y-4">
      {/* Mode toggle */}
      <div className="flex items-center justify-between">
        <Label className="flex items-center gap-2">
          <Clock className="h-4 w-4" />
          Schedule
        </Label>
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground">Cron mode</span>
          <Switch
            checked={cronMode}
            onCheckedChange={handleModeSwitch}
            disabled={cronMode && !canSwitchToUIMode}
          />
          <Code className="h-4 w-4 text-muted-foreground" />
        </div>
      </div>

      {/* Warning when cron can't be converted to UI mode */}
      {cronMode && !canSwitchToUIMode && (
        <p className="text-xs text-muted-foreground">
          This schedule uses advanced cron syntax that can't be edited in simple mode.
        </p>
      )}

      {cronMode ? (
        /* Cron mode */
        <div className="space-y-3">
          <Input
            value={cronInput}
            onChange={handleCronInputChange}
            placeholder="0 9 * * *"
            className="font-mono"
          />
          <p className="text-xs text-muted-foreground">
            Format: minute hour day-of-month month day-of-week (e.g., 0 9 * * 1-5 for weekdays at 9 AM UTC)
          </p>
          <p className="text-xs text-warning-foreground">
            Note: Cron times are in UTC
          </p>
        </div>
      ) : (
        /* UI mode */
        <div className="space-y-4">
          {/* Schedule type */}
          <Select value={scheduleType} onValueChange={handleScheduleTypeChange}>
            <SelectTrigger>
              <SelectValue placeholder="Select frequency" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="hourly">Hourly</SelectItem>
              <SelectItem value="daily">Daily</SelectItem>
              <SelectItem value="weekly">Weekly</SelectItem>
              <SelectItem value="monthly">Monthly</SelectItem>
            </SelectContent>
          </Select>

          {/* Time selector (not for hourly) */}
          {scheduleType !== 'hourly' && (
            <div className="flex items-center gap-2">
              <Label className="text-sm text-muted-foreground">At</Label>
              {/* Hour (12-hour format) */}
              <Select value={getHour12(hour).toString()} onValueChange={handleHour12Change}>
                <SelectTrigger className="w-[70px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {hour12Options.map(opt => (
                    <SelectItem key={opt.value} value={opt.value.toString()}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <span className="text-muted-foreground">:</span>
              {/* Minute */}
              <Select value={minute.toString()} onValueChange={handleMinuteChange}>
                <SelectTrigger className="w-[70px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {minuteOptions.map(opt => (
                    <SelectItem key={opt.value} value={opt.value.toString()}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {/* AM/PM */}
              <Select value={getAmPm(hour)} onValueChange={handleAmPmChange}>
                <SelectTrigger className="w-[70px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="AM">AM</SelectItem>
                  <SelectItem value="PM">PM</SelectItem>
                </SelectContent>
              </Select>
            </div>
          )}

          {/* Hour and minute selector for hourly */}
          {scheduleType === 'hourly' && (
            <div className="space-y-3">
              {/* Toggle button for hour selection */}
              {!showHourSelection ? (
                <div className="space-y-2">
                  <p className="text-sm text-muted-foreground">Runs every hour</p>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => setShowHourSelection(true)}
                  >
                    Select which hours to run
                  </Button>
                </div>
              ) : (
                <>
                  {/* Select specific hours */}
                  <div className="space-y-2">
                    <div className="flex items-center justify-between">
                      <Label className="text-sm text-muted-foreground">Select hours to run</Label>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          setShowHourSelection(false);
                          setSelectedHours([]);
                        }}
                        className="text-xs h-7"
                      >
                        Run every hour instead
                      </Button>
                    </div>
                    <div className="grid grid-cols-4 sm:grid-cols-6 gap-1">
                      {hourOptions.map(opt => (
                        <Button
                          key={opt.value}
                          type="button"
                          variant={selectedHours.includes(opt.value) ? 'default' : 'outline'}
                          size="sm"
                          onClick={() => toggleHour(opt.value)}
                          className="text-xs px-2"
                        >
                          {opt.label}
                        </Button>
                      ))}
                    </div>
                    {selectedHours.length === 0 && (
                      <p className="text-xs text-warning-foreground">Select at least one hour</p>
                    )}
                  </div>
                </>
              )}

              {/* Minute past the hour */}
              <div className="flex items-center gap-2">
                <Label className="text-sm text-muted-foreground">At</Label>
                <Select value={minute.toString()} onValueChange={handleMinuteChange}>
                  <SelectTrigger className="w-[80px]">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {minuteOptions.map(opt => (
                      <SelectItem key={opt.value} value={opt.value.toString()}>
                        :{opt.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <span className="text-sm text-muted-foreground">past the hour</span>
              </div>
            </div>
          )}

          {/* Weekday selector for weekly */}
          {scheduleType === 'weekly' && (
            <div className="space-y-2">
              <Label className="text-sm text-muted-foreground">On</Label>
              <div className="flex flex-wrap gap-1">
                {weekdayOptions.map(day => (
                  <Button
                    key={day.value}
                    type="button"
                    variant={weekdays.includes(day.value) ? 'default' : 'outline'}
                    size="sm"
                    onClick={() => toggleWeekday(day.value)}
                    className="w-10"
                  >
                    {day.label}
                  </Button>
                ))}
              </div>
              {weekdays.length === 0 && (
                <p className="text-xs text-warning-foreground">Select at least one day</p>
              )}
            </div>
          )}

          {/* Day of month selector for monthly */}
          {scheduleType === 'monthly' && (
            <div className="flex items-center gap-2">
              <Label className="text-sm text-muted-foreground">On the</Label>
              <Select value={dayOfMonth.toString()} onValueChange={handleDayOfMonthChange}>
                <SelectTrigger className="w-[100px]">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent className="max-h-[420px]">
                  {dayOfMonthOptions.map(opt => (
                    <SelectItem key={opt.value} value={opt.value.toString()}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <span className="text-sm text-muted-foreground">of each month</span>
            </div>
          )}
        </div>
      )}

      {/* Schedule description */}
      <div className={`rounded-lg p-3 text-sm ${cronDescription.valid ? 'bg-muted/50' : 'bg-error/10 border border-error-border'}`}>
        <div className="flex items-start gap-2">
          {!cronDescription.valid && <AlertCircle className="h-4 w-4 text-error-foreground mt-0.5 shrink-0" />}
          <div className="flex-1">
            <span className={cronDescription.valid ? 'text-foreground' : 'text-error-foreground'}>
              {cronDescription.description}
            </span>
            {cronDescription.valid && (
              <div className="text-xs text-muted-foreground mt-1">
                {cronMode ? (
                  <>Cron: <code className="bg-muted px-1 rounded">{cronInput}</code> (UTC)</>
                ) : (
                  'Times shown in your local timezone'
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
