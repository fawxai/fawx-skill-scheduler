/// Minimal 5-field cron expression parser.
///
/// Fields: minute hour day-of-month month day-of-week
/// Supports: `*`, `*/N`, `N`, `N,M`, `N-M`, and combinations thereof.

/// A parsed cron expression with pre-computed match sets for each field.
#[derive(Debug, Clone, PartialEq)]
pub struct CronExpr {
    pub minutes: Vec<u8>,   // 0-59
    pub hours: Vec<u8>,     // 0-23
    pub days: Vec<u8>,      // 1-31
    pub months: Vec<u8>,    // 1-12
    pub weekdays: Vec<u8>,  // 0-6 (0 = Sunday)
}

/// Parse error for cron expressions.
#[derive(Debug, PartialEq)]
pub struct CronParseError {
    pub message: String,
}

impl core::fmt::Display for CronParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cron parse error: {}", self.message)
    }
}

impl CronExpr {
    /// Parse a 5-field cron expression string.
    pub fn parse(expr: &str) -> Result<Self, CronParseError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronParseError {
                message: format!("expected 5 fields, got {}", fields.len()),
            });
        }

        let minutes = parse_field(fields[0], 0, 59)?;
        let hours = parse_field(fields[1], 0, 23)?;
        let days = parse_field(fields[2], 1, 31)?;
        let months = parse_field(fields[3], 1, 12)?;
        let weekdays = parse_field(fields[4], 0, 6)?;

        Ok(CronExpr {
            minutes,
            hours,
            days,
            months,
            weekdays,
        })
    }

    /// Check if this cron expression matches the given time components.
    pub fn matches(&self, minute: u8, hour: u8, day: u8, month: u8, weekday: u8) -> bool {
        self.minutes.contains(&minute)
            && self.hours.contains(&hour)
            && self.days.contains(&day)
            && self.months.contains(&month)
            && self.weekdays.contains(&weekday)
    }
}

/// Parse a single cron field into a sorted list of matching values.
fn parse_field(field: &str, min: u8, max: u8) -> Result<Vec<u8>, CronParseError> {
    let mut values = Vec::new();

    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(CronParseError {
                message: format!("empty element in field '{}'", field),
            });
        }

        if let Some(step_str) = part.strip_prefix("*/") {
            // */N — every N starting from min
            let step = parse_number(step_str, field)?;
            if step == 0 {
                return Err(CronParseError {
                    message: format!("step value must be > 0 in '{}'", field),
                });
            }
            let mut v = min;
            while v <= max {
                values.push(v);
                v = match v.checked_add(step) {
                    Some(next) => next,
                    None => break,
                };
            }
        } else if part == "*" {
            // * — all values
            for v in min..=max {
                values.push(v);
            }
        } else if part.contains('-') {
            // N-M — range
            let (start_str, end_str) = part.split_once('-').ok_or_else(|| CronParseError {
                message: format!("invalid range in '{}'", field),
            })?;
            let start = parse_number(start_str, field)?;
            let end = parse_number(end_str, field)?;
            if start < min || end > max || start > end {
                return Err(CronParseError {
                    message: format!("range {}-{} out of bounds ({}-{}) in '{}'", start, end, min, max, field),
                });
            }
            for v in start..=end {
                values.push(v);
            }
        } else {
            // N — exact value
            let v = parse_number(part, field)?;
            if v < min || v > max {
                return Err(CronParseError {
                    message: format!("value {} out of bounds ({}-{}) in '{}'", v, min, max, field),
                });
            }
            values.push(v);
        }
    }

    values.sort();
    values.dedup();
    Ok(values)
}

/// Parse a numeric string, returning a friendly error.
fn parse_number(s: &str, field: &str) -> Result<u8, CronParseError> {
    s.parse::<u8>().map_err(|_| CronParseError {
        message: format!("invalid number '{}' in field '{}'", s, field),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard() {
        let expr = CronExpr::parse("* * * * *").unwrap();
        assert_eq!(expr.minutes.len(), 60);
        assert_eq!(expr.hours.len(), 24);
        assert_eq!(expr.days.len(), 31);
        assert_eq!(expr.months.len(), 12);
        assert_eq!(expr.weekdays.len(), 7);
    }

    #[test]
    fn test_exact_values() {
        let expr = CronExpr::parse("30 9 15 6 3").unwrap();
        assert_eq!(expr.minutes, vec![30]);
        assert_eq!(expr.hours, vec![9]);
        assert_eq!(expr.days, vec![15]);
        assert_eq!(expr.months, vec![6]);
        assert_eq!(expr.weekdays, vec![3]);
    }

    #[test]
    fn test_step() {
        let expr = CronExpr::parse("*/15 */6 * * *").unwrap();
        assert_eq!(expr.minutes, vec![0, 15, 30, 45]);
        assert_eq!(expr.hours, vec![0, 6, 12, 18]);
    }

    #[test]
    fn test_range() {
        let expr = CronExpr::parse("0 9-17 * * *").unwrap();
        assert_eq!(expr.hours, vec![9, 10, 11, 12, 13, 14, 15, 16, 17]);
    }

    #[test]
    fn test_list() {
        let expr = CronExpr::parse("0 9,12,18 * * *").unwrap();
        assert_eq!(expr.hours, vec![9, 12, 18]);
    }

    #[test]
    fn test_combined_list_and_range() {
        let expr = CronExpr::parse("0,30 9-11 * * 1,3,5").unwrap();
        assert_eq!(expr.minutes, vec![0, 30]);
        assert_eq!(expr.hours, vec![9, 10, 11]);
        assert_eq!(expr.weekdays, vec![1, 3, 5]);
    }

    #[test]
    fn test_matches() {
        let expr = CronExpr::parse("0 9 * * 1-5").unwrap();
        // Monday 9:00 — should match
        assert!(expr.matches(0, 9, 15, 6, 1));
        // Sunday 9:00 — should not match (weekday 0)
        assert!(!expr.matches(0, 9, 15, 6, 0));
        // Monday 10:00 — should not match
        assert!(!expr.matches(0, 10, 15, 6, 1));
    }

    #[test]
    fn test_invalid_field_count() {
        assert!(CronExpr::parse("* * *").is_err());
        assert!(CronExpr::parse("* * * * * *").is_err());
    }

    #[test]
    fn test_out_of_bounds() {
        assert!(CronExpr::parse("60 * * * *").is_err());
        assert!(CronExpr::parse("* 24 * * *").is_err());
        assert!(CronExpr::parse("* * 0 * *").is_err());
        assert!(CronExpr::parse("* * * 13 *").is_err());
        assert!(CronExpr::parse("* * * * 7").is_err());
    }

    #[test]
    fn test_step_zero() {
        assert!(CronExpr::parse("*/0 * * * *").is_err());
    }
}
