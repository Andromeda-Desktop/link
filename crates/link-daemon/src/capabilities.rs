//! What the running `link-session`s can do. More than one can report (a second login, a session that restarts
//! before the old one leaves); the latest report of a session still on the bus is the one phones see.

#[derive(Default)]
pub struct Reports(Vec<(String, Vec<String>)>);

impl Reports {
    /// The capabilities phones see: the latest report of a session still present.
    pub fn current(&self) -> &[String] {
        self.0.last().map_or(&[], |(_, can)| can.as_slice())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Records `owner`'s report, replacing its earlier one, as the latest.
    pub fn report(&mut self, owner: String, can: Vec<String>) {
        self.0.retain(|(current, _)| *current != owner);
        self.0.push((owner, can));
    }

    /// Forgets `owner`; true when it held the current report.
    pub fn gone(&mut self, owner: &str) -> bool {
        let was_current = self.0.last().is_some_and(|(current, _)| current == owner);
        self.0.retain(|(current, _)| current != owner);
        was_current
    }
}

#[cfg(test)]
mod tests {
    use super::Reports;

    #[test]
    fn the_latest_live_report_is_current_after_any_order_of_reports_and_leaves() {
        let owners = [":1.1", ":1.2", ":1.3"];
        let mut seed = 0x2545_f491_u32;
        for _ in 0..2000 {
            let mut reports = Reports::default();
            let mut model: Vec<(&str, u32)> = Vec::new();
            for step in 0..12 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let owner = owners[(seed % 3) as usize];
                if seed & 8 == 0 {
                    reports.report(owner.to_owned(), vec![format!("can-{step}")]);
                    model.retain(|(o, _)| *o != owner);
                    model.push((owner, step));
                } else {
                    let was_current = model.last().is_some_and(|(o, _)| *o == owner);
                    assert_eq!(reports.gone(owner), was_current);
                    model.retain(|(o, _)| *o != owner);
                }
                let expected: Vec<String> = model.last().map(|(_, s)| vec![format!("can-{s}")]).unwrap_or_default();
                assert_eq!(reports.current(), expected.as_slice());
                assert_eq!(reports.is_empty(), model.is_empty());
            }
        }
    }
}
