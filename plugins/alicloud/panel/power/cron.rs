//! Bounded five-field cron evaluation using fixed UTC offsets.

const DAYS: i64 = 366 * 5;

#[derive(Debug)]
pub(super) struct Cron {
    fields: Vec<Vec<u32>>,
    any_day: bool,
    any_weekday: bool,
}

impl Cron {
    pub fn parse(expression: &str) -> Result<Self, &'static str> {
        if expression.len() > 256 {
            return Err("Cron 表达式过长");
        }
        let parts: Vec<_> = expression.split_whitespace().collect();
        if parts.len() != 5 {
            return Err("Cron 需要五字段：分 时 日 月 周");
        }
        let bounds = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
        let fields = parts
            .iter()
            .zip(bounds)
            .map(|(part, (min, max))| field(part, min, max))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            fields,
            any_day: parts[2].starts_with('*'),
            any_weekday: parts[4].starts_with('*'),
        })
    }

    fn day_matches(&self, day: i64) -> bool {
        let date = crate::cloud_api::signing::iso_time(day * 86400);
        let month: u32 = date[5..7].parse().expect("formatted month");
        let date: u32 = date[8..10].parse().expect("formatted day");
        let weekday = (day + 4).rem_euclid(7) as u32;
        let dom = self.fields[2].contains(&date);
        let dow =
            self.fields[4].contains(&weekday) || (weekday == 0 && self.fields[4].contains(&7));
        self.fields[3].contains(&month)
            && if self.any_day || self.any_weekday {
                dom && dow
            } else {
                dom || dow
            }
    }

    /// Includes `at` for past events, excludes it for future previews.
    pub fn occurrence(&self, at: i64, offset: i64, future: bool) -> Option<i64> {
        let local = at.checked_add(offset.checked_mul(60)?)?;
        let day = local.div_euclid(86400);
        for distance in 0..=DAYS {
            let day = if future {
                day + distance
            } else {
                day - distance
            };
            if !self.day_matches(day) {
                continue;
            }
            let mut found = None;
            for hour in &self.fields[1] {
                for minute in &self.fields[0] {
                    let candidate = day * 86400 + i64::from(hour * 60 + minute) * 60 - offset * 60;
                    if future && candidate > at {
                        return Some(candidate);
                    }
                    if !future && candidate <= at {
                        found = Some(candidate);
                    }
                }
            }
            if found.is_some() {
                return found;
            }
        }
        None
    }

    pub fn preview(&self, now: i64, offset: i64) -> Vec<i64> {
        let mut cursor = now;
        (0..5)
            .filter_map(|_| {
                let next = self.occurrence(cursor, offset, true)?;
                cursor = next;
                Some(next)
            })
            .collect()
    }
}

fn field(input: &str, min: u32, max: u32) -> Result<Vec<u32>, &'static str> {
    let invalid = "Cron 仅支持范围内数字、*、列表、范围和正整数步长";
    let number = |value: &str| -> Result<u32, &'static str> {
        if value.is_empty() || !value.bytes().all(|c| c.is_ascii_digit()) {
            return Err(invalid);
        }
        value.parse().map_err(|_| invalid)
    };
    let mut selected = Vec::new();
    for part in input.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, number(step)?),
            None => (part, 1),
        };
        if step == 0 || step > max + 1 {
            return Err(invalid);
        }
        let (from, to) = if range == "*" {
            (min, max)
        } else if let Some((from, to)) = range.split_once('-') {
            (number(from)?, number(to)?)
        } else {
            let value = number(range)?;
            (value, if part.contains('/') { max } else { value })
        };
        if from < min || to > max || from > to {
            return Err(invalid);
        }
        selected.extend((from..=to).step_by(step as usize));
    }
    selected.sort_unstable();
    selected.dedup();
    if selected.is_empty() {
        return Err(invalid);
    }
    Ok(selected)
}

#[cfg(test)]
mod tests;
