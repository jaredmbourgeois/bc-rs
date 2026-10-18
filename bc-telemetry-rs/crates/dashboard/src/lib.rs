#![doc = include_str!("../README.md")]
#![deny(missing_docs)]
// A discard or a lint exception in this crate says why, and stops compiling
// quietly the day it stops being true: `#[expect]` is fulfilled-or-warn, and
// these three make `let _ =` and `#[allow]` say so at the site.
#![warn(
    clippy::let_underscore_must_use,
    clippy::allow_attributes,
    clippy::allow_attributes_without_reason
)]

use std::fmt;

use bc_json::{JsonWriteError, JsonWriter};

/// The dashboard schema version this model writes: the one Grafana's published
/// dashboard schema defaults to at the revision this crate vendors
/// (`schema/dashboard.jsonschema.json`).
const SCHEMA_VERSION: i64 = 42;

/// Grafana's type id for its PostgreSQL data source.
const POSTGRESQL_DATA_SOURCE_TYPE: &str = "grafana-postgresql-datasource";

/// The width of Grafana's grid, in columns.
const GRID_COLUMNS: i64 = 24;

/// The longest uid Grafana stores.
const MAXIMUM_UID_LENGTH: usize = 40;

/// Why a dashboard's identity was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DashboardIdError {
    /// The uid is empty.
    Empty,
    /// The uid is longer than Grafana's 40 characters.
    TooLong {
        /// The uid as given.
        uid: String,
    },
    /// The uid holds a character other than a letter, a digit, `-` or `_`.
    Character {
        /// The uid as given.
        uid: String,
        /// The first character Grafana would refuse.
        character: char,
    },
}

impl fmt::Display for DashboardIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a dashboard uid is empty"),
            Self::TooLong { uid } => write!(
                formatter,
                "the dashboard uid `{uid}` is {} characters, past Grafana's {MAXIMUM_UID_LENGTH}",
                uid.chars().count()
            ),
            Self::Character { uid, character } => write!(
                formatter,
                "the dashboard uid `{uid}` holds `{character}`; a uid is letters, digits, `-` and `_`"
            ),
        }
    }
}

impl std::error::Error for DashboardIdError {}

/// A dashboard's identity in Grafana: its uid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardId(String);

impl DashboardId {
    /// The dashboard whose uid is `uid`.
    ///
    /// # Errors
    ///
    /// The uid is empty, longer than Grafana's 40 characters, or holds a
    /// character other than a letter, a digit, `-` or `_`.
    pub fn new(uid: impl Into<String>) -> Result<Self, DashboardIdError> {
        let uid = uid.into();
        if uid.is_empty() {
            return Err(DashboardIdError::Empty);
        }
        if uid.chars().count() > MAXIMUM_UID_LENGTH {
            return Err(DashboardIdError::TooLong { uid });
        }
        let refused = uid.chars().find(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        });
        if let Some(character) = refused {
            return Err(DashboardIdError::Character { uid, character });
        }
        Ok(Self(uid))
    }

    /// The uid.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The Grafana data source a query reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSource {
    uid: String,
}

impl DataSource {
    /// Grafana's PostgreSQL data source with the uid `uid`: a database holding
    /// the bc-telemetry layout.
    #[must_use]
    pub fn postgresql(uid: impl Into<String>) -> Self {
        Self { uid: uid.into() }
    }

    /// Writes `{"type": …, "uid": …}` under `key`.
    fn write(&self, json: &mut JsonWriter, key: &str) {
        json.nested_object(key, |json| {
            json.field("type", POSTGRESQL_DATA_SOURCE_TYPE);
            json.field("uid", self.uid.as_str());
        });
    }
}

/// What a measurement is in: the closed set a metric's declared `unit` names,
/// each one Grafana unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A plain number: a count, a size in items.
    Unitless,
    /// Milliseconds.
    Milliseconds,
    /// Seconds.
    Seconds,
    /// Bytes.
    Bytes,
    /// A percentage, from 0 to 100.
    Percent,
}

/// What a query's result is in: a unit, or a unit per second (a rate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultUnit {
    /// The unit itself: a sum, an average, an extreme, a percentile — or, for
    /// [`Unit::Unitless`], a count.
    Of(Unit),
    /// The unit per second: a rate.
    PerSecond(Unit),
}

impl ResultUnit {
    /// Grafana's id for this unit. A rate of something Grafana has no rate unit
    /// for is a custom suffix, which Grafana prints after the number.
    #[must_use]
    pub const fn grafana_id(self) -> &'static str {
        match self {
            Self::Of(Unit::Unitless) => "none",
            Self::Of(Unit::Milliseconds) => "ms",
            Self::Of(Unit::Seconds) => "s",
            Self::Of(Unit::Bytes) => "bytes",
            Self::Of(Unit::Percent) => "percent",
            Self::PerSecond(Unit::Unitless) => "cps",
            Self::PerSecond(Unit::Bytes) => "binBps",
            Self::PerSecond(Unit::Milliseconds) => "suffix: ms/s",
            Self::PerSecond(Unit::Seconds) => "suffix: s/s",
            Self::PerSecond(Unit::Percent) => "suffix: %/s",
        }
    }
}

/// A query whose answer is a measurement over time: what a timeseries, a stat
/// or a table panel shows. Generated per named query.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasureQuery {
    description: String,
    sql: String,
    unit: ResultUnit,
    data_source: DataSource,
}

impl MeasureQuery {
    /// The query whose statement is `sql`, read from `data_source`, answering
    /// in `unit`.
    #[must_use]
    pub fn new(
        description: impl Into<String>,
        sql: impl Into<String>,
        unit: ResultUnit,
        data_source: DataSource,
    ) -> Self {
        Self {
            description: description.into(),
            sql: sql.into(),
            unit,
            data_source,
        }
    }
}

/// A query that reads a log's or an error's records, newest first: what a logs
/// panel shows.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordsQuery {
    description: String,
    sql: String,
    data_source: DataSource,
}

impl RecordsQuery {
    /// The records `sql` reads from `data_source`.
    #[must_use]
    pub fn new(
        description: impl Into<String>,
        sql: impl Into<String>,
        data_source: DataSource,
    ) -> Self {
        Self {
            description: description.into(),
            sql: sql.into(),
            data_source,
        }
    }
}

/// A color a threshold paints, from Grafana's named palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Yellow.
    Yellow,
    /// Orange.
    Orange,
    /// Red.
    Red,
    /// Purple.
    Purple,
}

impl Color {
    const fn grafana_name(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Yellow => "yellow",
            Self::Orange => "orange",
            Self::Red => "red",
            Self::Purple => "purple",
        }
    }
}

/// The colors a panel's values take: one from the bottom, then one above each
/// step.
#[derive(Debug, Clone, PartialEq)]
pub struct Thresholds {
    base: Color,
    steps: Vec<(f64, Color)>,
}

impl Thresholds {
    /// Every value is `base` until a step says otherwise.
    #[must_use]
    pub fn starting(base: Color) -> Self {
        Self {
            base,
            steps: Vec::new(),
        }
    }

    /// From `value` up, `color`. Steps are kept in the order given, which
    /// should be ascending: Grafana reads them so.
    #[must_use]
    pub fn above(mut self, value: f64, color: Color) -> Self {
        self.steps.push((value, color));
        self
    }

    fn write(&self, json: &mut JsonWriter) {
        json.nested_object("thresholds", |json| {
            json.field("mode", "absolute");
            json.array("steps", |json| {
                json.array_value(|json| {
                    json.object(|json| {
                        json.field_null("value");
                        json.field("color", self.base.grafana_name());
                    });
                });
                for (value, color) in &self.steps {
                    json.array_value(|json| {
                        json.object(|json| {
                            json.field("value", *value);
                            json.field("color", color.grafana_name());
                        });
                    });
                }
            });
        });
    }
}

/// The panel types that show a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasureView {
    Timeseries,
    Stat,
    Table,
}

impl MeasureView {
    const fn plugin(self) -> &'static str {
        match self {
            Self::Timeseries => "timeseries",
            Self::Stat => "stat",
            Self::Table => "table",
        }
    }

    /// How Grafana's PostgreSQL data source shapes the answer for this view.
    const fn format(self) -> &'static str {
        match self {
            Self::Timeseries | Self::Stat => "time_series",
            Self::Table => "table",
        }
    }
}

/// What every measure panel holds.
#[derive(Debug, Clone, PartialEq)]
struct MeasurePanel {
    view: MeasureView,
    title: String,
    query: MeasureQuery,
    thresholds: Option<Thresholds>,
}

/// A panel that charts a measurement over time.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeseries(MeasurePanel);

/// A panel that shows a measurement as one number.
#[derive(Debug, Clone, PartialEq)]
pub struct Stat(MeasurePanel);

/// A panel that shows a measurement as a table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table(MeasurePanel);

/// A panel that shows a log's or an error's records.
#[derive(Debug, Clone, PartialEq)]
pub struct Logs {
    title: String,
    query: RecordsQuery,
}

impl Timeseries {
    /// A chart titled `title` of what `query` answers.
    #[must_use]
    pub fn new(title: impl Into<String>, query: MeasureQuery) -> Self {
        Self(MeasurePanel {
            view: MeasureView::Timeseries,
            title: title.into(),
            query,
            thresholds: None,
        })
    }

    /// As [`Timeseries::new`], its values colored by `thresholds`.
    #[must_use]
    pub fn with_thresholds(
        title: impl Into<String>,
        query: MeasureQuery,
        thresholds: Thresholds,
    ) -> Self {
        Self(MeasurePanel {
            view: MeasureView::Timeseries,
            title: title.into(),
            query,
            thresholds: Some(thresholds),
        })
    }
}

impl Stat {
    /// One number titled `title`: what `query` answers.
    #[must_use]
    pub fn new(title: impl Into<String>, query: MeasureQuery) -> Self {
        Self(MeasurePanel {
            view: MeasureView::Stat,
            title: title.into(),
            query,
            thresholds: None,
        })
    }

    /// As [`Stat::new`], its number colored by `thresholds`.
    #[must_use]
    pub fn with_thresholds(
        title: impl Into<String>,
        query: MeasureQuery,
        thresholds: Thresholds,
    ) -> Self {
        Self(MeasurePanel {
            view: MeasureView::Stat,
            title: title.into(),
            query,
            thresholds: Some(thresholds),
        })
    }
}

impl Table {
    /// A table titled `title` of what `query` answers.
    #[must_use]
    pub fn new(title: impl Into<String>, query: MeasureQuery) -> Self {
        Self(MeasurePanel {
            view: MeasureView::Table,
            title: title.into(),
            query,
            thresholds: None,
        })
    }
}

impl Logs {
    /// The records `query` reads, titled `title`.
    #[must_use]
    pub fn new(title: impl Into<String>, query: RecordsQuery) -> Self {
        Self {
            title: title.into(),
            query,
        }
    }
}

/// Any panel a dashboard or a row holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Panel {
    /// A chart over time.
    Timeseries(Timeseries),
    /// One number.
    Stat(Stat),
    /// A table.
    Table(Table),
    /// A log's or an error's records.
    Logs(Logs),
}

impl From<Timeseries> for Panel {
    fn from(panel: Timeseries) -> Self {
        Self::Timeseries(panel)
    }
}

impl From<Stat> for Panel {
    fn from(panel: Stat) -> Self {
        Self::Stat(panel)
    }
}

impl From<Table> for Panel {
    fn from(panel: Table) -> Self {
        Self::Table(panel)
    }
}

impl From<Logs> for Panel {
    fn from(panel: Logs) -> Self {
        Self::Logs(panel)
    }
}

/// A titled group of panels.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    title: String,
    panels: Vec<Panel>,
}

impl Row {
    /// An empty row titled `title`.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            panels: Vec::new(),
        }
    }

    /// Adds `panel` after the row's others.
    #[must_use]
    pub fn panel(mut self, panel: impl Into<Panel>) -> Self {
        self.panels.push(panel.into());
        self
    }
}

/// A dashboard: panels of its own, then rows of panels.
///
/// Grafana places a panel that follows a row inside that row, so a dashboard's
/// own panels are always laid out before its first row, whatever order they
/// were added in.
#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    id: DashboardId,
    title: String,
    panels: Vec<Panel>,
    rows: Vec<Row>,
}

impl Dashboard {
    /// An empty dashboard.
    #[must_use]
    pub fn new(id: DashboardId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            panels: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// Adds `panel` after the dashboard's own others, above every row.
    #[must_use]
    pub fn panel(mut self, panel: impl Into<Panel>) -> Self {
        self.panels.push(panel.into());
        self
    }

    /// Adds `row` after the others.
    #[must_use]
    pub fn row(mut self, row: Row) -> Self {
        self.rows.push(row);
        self
    }

    /// The dashboard as Grafana's classic dashboard JSON, pretty-printed and
    /// ending in a newline: what Grafana's file provisioning reads.
    ///
    /// # Errors
    ///
    /// A threshold that is not a finite number, which JSON cannot hold.
    pub fn to_json(&self) -> Result<String, JsonWriteError> {
        let mut layout = Layout::starting();
        let mut json = JsonWriter::pretty();
        json.object(|json| {
            json.field("uid", self.id.as_str());
            json.field("title", self.title.as_str());
            json.field("schemaVersion", SCHEMA_VERSION);
            json.array("panels", |json| {
                for panel in &self.panels {
                    write_panel(json, panel, &mut layout);
                }
                for row in &self.rows {
                    write_row(json, row, &mut layout);
                    for panel in &row.panels {
                        write_panel(json, panel, &mut layout);
                    }
                }
            });
            json.nested_object("templating", |json| {
                json.array("list", |_| {});
            });
            json.nested_object("annotations", |json| {
                json.array("list", |_| {});
            });
        });
        let mut text = json.finish()?;
        text.push('\n');
        Ok(text)
    }
}

/// Where the next panel goes, and the id it takes.
///
/// Panels fill Grafana's 24-column grid left to right: a measure panel takes
/// half a line, a logs panel a whole one, and a row a line of its own.
struct Layout {
    next_id: i64,
    x: i64,
    y: i64,
    line_height: i64,
}

/// Where a panel was placed.
struct Placement {
    id: i64,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl Layout {
    const fn starting() -> Self {
        Self {
            next_id: 1,
            x: 0,
            y: 0,
            line_height: 0,
        }
    }

    /// Moves below the current line, unless nothing is on it yet.
    fn new_line(&mut self) {
        if self.x == 0 {
            return;
        }
        self.y += self.line_height;
        self.x = 0;
        self.line_height = 0;
    }

    fn place(&mut self, width: i64, height: i64) -> Placement {
        if self.x + width > GRID_COLUMNS {
            self.new_line();
        }
        let placement = Placement {
            id: self.next_id,
            x: self.x,
            y: self.y,
            width,
            height,
        };
        self.next_id += 1;
        self.x += width;
        self.line_height = self.line_height.max(height);
        placement
    }

    /// A row takes a line of its own; what follows starts below it.
    fn place_row(&mut self) -> Placement {
        self.new_line();
        let placement = self.place(GRID_COLUMNS, 1);
        self.new_line();
        placement
    }
}

fn write_placement(json: &mut JsonWriter, placement: &Placement) {
    json.field("id", placement.id);
    json.nested_object("gridPos", |json| {
        json.field("h", placement.height);
        json.field("w", placement.width);
        json.field("x", placement.x);
        json.field("y", placement.y);
    });
}

fn write_row(json: &mut JsonWriter, row: &Row, layout: &mut Layout) {
    let placement = layout.place_row();
    json.array_value(|json| {
        json.object(|json| {
            json.field("type", "row");
            json.field("title", row.title.as_str());
            json.field("collapsed", false);
            write_placement(json, &placement);
            // An open row's panels are the dashboard's, listed after it.
            json.array("panels", |_| {});
        });
    });
}

/// The one target a panel reads: its query, as Grafana's PostgreSQL data source
/// takes raw SQL.
fn write_target(json: &mut JsonWriter, sql: &str, format: &str, data_source: &DataSource) {
    json.array("targets", |json| {
        json.array_value(|json| {
            json.object(|json| {
                json.field("refId", "A");
                data_source.write(json, "datasource");
                json.field("editorMode", "code");
                json.field("format", format);
                json.field("rawQuery", true);
                json.field("rawSql", sql);
            });
        });
    });
}

fn write_panel(json: &mut JsonWriter, panel: &Panel, layout: &mut Layout) {
    match panel {
        Panel::Timeseries(Timeseries(measure))
        | Panel::Stat(Stat(measure))
        | Panel::Table(Table(measure)) => {
            let placement = layout.place(GRID_COLUMNS / 2, 8);
            json.array_value(|json| {
                json.object(|json| {
                    json.field("type", measure.view.plugin());
                    json.field("title", measure.title.as_str());
                    json.field("description", measure.query.description.as_str());
                    write_placement(json, &placement);
                    measure.query.data_source.write(json, "datasource");
                    write_target(
                        json,
                        &measure.query.sql,
                        measure.view.format(),
                        &measure.query.data_source,
                    );
                    json.nested_object("fieldConfig", |json| {
                        json.nested_object("defaults", |json| {
                            json.field("unit", measure.query.unit.grafana_id());
                            if let Some(thresholds) = &measure.thresholds {
                                thresholds.write(json);
                            }
                        });
                        json.array("overrides", |_| {});
                    });
                    json.nested_object("options", |_| {});
                });
            });
        }
        Panel::Logs(logs) => {
            let placement = layout.place(GRID_COLUMNS, 10);
            json.array_value(|json| {
                json.object(|json| {
                    json.field("type", "logs");
                    json.field("title", logs.title.as_str());
                    json.field("description", logs.query.description.as_str());
                    write_placement(json, &placement);
                    logs.query.data_source.write(json, "datasource");
                    write_target(json, &logs.query.sql, "table", &logs.query.data_source);
                    json.nested_object("fieldConfig", |json| {
                        json.nested_object("defaults", |_| {});
                        json.array("overrides", |_| {});
                    });
                    json.nested_object("options", |_| {});
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, ResultUnit, Unit};

    #[test]
    fn every_unit_has_its_grafana_id() {
        let expected = [
            (ResultUnit::Of(Unit::Unitless), "none"),
            (ResultUnit::Of(Unit::Milliseconds), "ms"),
            (ResultUnit::Of(Unit::Seconds), "s"),
            (ResultUnit::Of(Unit::Bytes), "bytes"),
            (ResultUnit::Of(Unit::Percent), "percent"),
            (ResultUnit::PerSecond(Unit::Unitless), "cps"),
            (ResultUnit::PerSecond(Unit::Bytes), "binBps"),
            (ResultUnit::PerSecond(Unit::Milliseconds), "suffix: ms/s"),
            (ResultUnit::PerSecond(Unit::Seconds), "suffix: s/s"),
            (ResultUnit::PerSecond(Unit::Percent), "suffix: %/s"),
        ];
        for (unit, id) in expected {
            assert_eq!(unit.grafana_id(), id, "{unit:?}");
        }
    }

    #[test]
    fn panels_fill_the_grid_left_to_right_and_a_row_takes_a_line() {
        let mut layout = Layout::starting();
        let first = layout.place(12, 8);
        let second = layout.place(12, 8);
        let third = layout.place(24, 10);
        let row = layout.place_row();
        let after_row = layout.place(12, 8);
        let placed: Vec<(i64, i64, i64)> = [first, second, third, row, after_row]
            .iter()
            .map(|placement| (placement.id, placement.x, placement.y))
            .collect();
        assert_eq!(
            placed,
            [(1, 0, 0), (2, 12, 0), (3, 0, 8), (4, 0, 18), (5, 0, 19)]
        );
    }
}
