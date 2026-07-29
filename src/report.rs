//! Read and summarize pprof profiles for terminal and Markdown consumers.

use std::collections::{HashMap, HashSet};
use std::io::Read;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

use crate::pprof::{Function, Location, PprofProfile, Sample};

/// Flat and cumulative cost attributed to one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameStat {
    pub function: String,
    pub file: String,
    pub line: i64,
    pub flat: i64,
    pub cumulative: i64,
}

/// One frame in a ranked stack, ordered from root to leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportFrame {
    pub function: String,
    pub file: String,
    pub line: i64,
}

/// Cost attributed to one complete call stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackStat {
    pub frames: Vec<ReportFrame>,
    pub value: i64,
}

/// A focused report view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportView {
    /// Metadata, observations, top frames, and top stacks.
    Summary,
    /// Functions ranked by self/flat cost.
    TopFrames,
    /// Functions ranked by cumulative cost.
    TopCumulative,
    /// Complete call stacks ranked by cost.
    TopStacks,
    /// A root-to-leaf hot call tree.
    CallTree,
}

/// Output syntax for a rendered report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Terminal,
    Markdown,
}

/// Controls report size and filtering.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub view: ReportView,
    pub format: ReportFormat,
    pub limit: usize,
    /// Hide rows and tree nodes below this percentage of the profile total.
    pub min_percent: f64,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            view: ReportView::Summary,
            format: ReportFormat::Terminal,
            limit: 10,
            min_percent: 0.0,
        }
    }
}

/// An analyzed pprof profile, ranked by actionable flat cost.
#[derive(Debug, Clone)]
pub struct ProfileReport {
    sample_type: String,
    sample_unit: String,
    available_sample_types: Vec<String>,
    duration_nanos: i64,
    sample_records: usize,
    total_value: i64,
    top_frames: Vec<FrameStat>,
    top_stacks: Vec<StackStat>,
}

impl ProfileReport {
    /// Decode a raw or gzip-compressed pprof protobuf and analyze one sample type.
    ///
    /// `sample_type` accepts a sample type name or zero-based index. When it is
    /// absent, pprof's `default_sample_type` is used, falling back to the last
    /// sample type, matching `go tool pprof`.
    pub fn from_pprof(data: &[u8], sample_type: Option<&str>) -> Result<Self> {
        let data = decompress_if_needed(data)?;
        let profile = PprofProfile::decode(&data)?;
        Self::from_profile(&profile, sample_type)
    }

    /// Analyze an already-decoded canonical pprof profile.
    pub fn from_profile(profile: &PprofProfile, sample_type: Option<&str>) -> Result<Self> {
        let sample_index = sample_index(profile, sample_type)?;
        let selected = profile
            .value_types
            .get(sample_index)
            .context("pprof profile has no sample types")?;
        let sample_type = profile_string(profile, selected.r#type).to_owned();
        let sample_unit = profile_string(profile, selected.unit).to_owned();
        let available_sample_types = profile
            .value_types
            .iter()
            .map(|sample| profile_string(profile, sample.r#type).to_owned())
            .collect();
        let sample_records = profile.samples.len();
        let locations: HashMap<_, _> = profile
            .locations
            .iter()
            .map(|location| (location.id, location))
            .collect();
        let functions: HashMap<_, _> = profile
            .functions
            .iter()
            .map(|function| (function.id, function))
            .collect();

        let mut stats: HashMap<FrameIdentity, FrameStat> = HashMap::new();
        let mut stacks: HashMap<Vec<FrameIdentity>, StackStat> = HashMap::new();
        let mut total_value = 0i64;
        for sample in &profile.samples {
            let Some(&value) = sample.values.get(sample_index) else {
                continue;
            };
            total_value = total_value.saturating_add(value.saturating_abs());

            let frames = resolve_stack(profile, sample, &locations, &functions);
            let stack_key: Vec<_> = frames.iter().map(|frame| frame.identity).collect();
            if !stack_key.is_empty() {
                let stack = stacks.entry(stack_key).or_insert_with(|| StackStat {
                    frames: frames.iter().map(ResolvedFrame::report_frame).collect(),
                    value: 0,
                });
                stack.value = stack.value.saturating_add(value);
            }
            if let Some(leaf) = frames.last() {
                let stat = stats.entry(leaf.identity).or_insert_with(|| leaf.stat());
                stat.flat = stat.flat.saturating_add(value);
            }

            let mut seen = HashSet::new();
            for frame in frames {
                if seen.insert(frame.identity) {
                    let stat = stats.entry(frame.identity).or_insert_with(|| frame.stat());
                    stat.cumulative = stat.cumulative.saturating_add(value);
                }
            }
        }

        let mut top_frames: Vec<_> = stats.into_values().collect();
        top_frames.sort_by(|a, b| {
            b.flat
                .saturating_abs()
                .cmp(&a.flat.saturating_abs())
                .then_with(|| {
                    b.cumulative
                        .saturating_abs()
                        .cmp(&a.cumulative.saturating_abs())
                })
                .then_with(|| a.function.cmp(&b.function))
        });
        let mut top_stacks: Vec<_> = stacks.into_values().collect();
        top_stacks.sort_by(|a, b| {
            b.value
                .saturating_abs()
                .cmp(&a.value.saturating_abs())
                .then_with(|| {
                    let a = a
                        .frames
                        .iter()
                        .map(|frame| frame.function.as_str())
                        .collect::<Vec<_>>();
                    let b = b
                        .frames
                        .iter()
                        .map(|frame| frame.function.as_str())
                        .collect::<Vec<_>>();
                    a.cmp(&b)
                })
        });

        Ok(Self {
            sample_type,
            sample_unit,
            available_sample_types,
            duration_nanos: profile.duration_nanos,
            sample_records,
            total_value,
            top_frames,
            top_stacks,
        })
    }

    pub fn sample_type(&self) -> (&str, &str) {
        (&self.sample_type, &self.sample_unit)
    }

    pub fn total_value(&self) -> i64 {
        self.total_value
    }

    pub fn top_frames(&self) -> &[FrameStat] {
        &self.top_frames
    }

    pub fn top_stacks(&self) -> &[StackStat] {
        &self.top_stacks
    }

    /// Render one terminal- or Markdown-oriented profile view.
    pub fn render(&self, options: &RenderOptions) -> String {
        match (options.view, options.format) {
            (ReportView::Summary, ReportFormat::Markdown) => self.render_markdown_summary(options),
            (ReportView::Summary, ReportFormat::Terminal) => self.render_terminal_summary(options),
            (ReportView::TopFrames, format) => {
                self.render_frames(options, format, FrameOrder::Flat)
            }
            (ReportView::TopCumulative, format) => {
                self.render_frames(options, format, FrameOrder::Cumulative)
            }
            (ReportView::TopStacks, format) => self.render_stacks(options, format),
            (ReportView::CallTree, format) => self.render_tree(options, format),
        }
    }

    fn render_markdown_summary(&self, options: &RenderOptions) -> String {
        let mut output = String::new();
        output.push_str("# pprof Hotspot Report\n\n");
        output.push_str(&format!(
            "- Sample type: {} ({})\n- Available sample types: {}\n- Duration: {}\n- Sample records: {}\n- Total cost: {}\n- Unique frames: {}\n- Unique stacks: {}\n\n",
            markdown_code(&self.sample_type),
            markdown_code(&self.sample_unit),
            self.available_sample_types
                .iter()
                .map(|sample_type| markdown_code(sample_type))
                .collect::<Vec<_>>()
                .join(", "),
            format_value(self.duration_nanos, "nanoseconds"),
            self.sample_records,
            self.format_value(self.total_value),
            self.top_frames.len(),
            self.top_stacks.len()
        ));
        output.push_str("## Key observations\n\n");
        for observation in self.observations(options.limit) {
            output.push_str("- ");
            output.push_str(&observation);
            output.push('\n');
        }
        output.push('\n');
        output.push_str("## Top frames\n\n");
        output.push_str(&self.render_frames(options, ReportFormat::Markdown, FrameOrder::Flat));
        output.push('\n');
        output.push_str("## Top stacks\n\n");
        output.push_str(&self.render_stacks(options, ReportFormat::Markdown));
        output
    }

    fn render_terminal_summary(&self, options: &RenderOptions) -> String {
        let mut output = String::new();
        output.push_str("PPROF HOTSPOT REPORT\n");
        output.push_str(&format!(
            "Sample type: {} ({})\nAvailable sample types: {}\nDuration: {}\nSample records: {}\nTotal cost: {}\nUnique frames: {}\nUnique stacks: {}\n\n",
            self.sample_type,
            self.sample_unit,
            self.available_sample_types.join(", "),
            format_value(self.duration_nanos, "nanoseconds"),
            self.sample_records,
            self.format_value(self.total_value),
            self.top_frames.len(),
            self.top_stacks.len()
        ));
        output.push_str("KEY OBSERVATIONS\n");
        for observation in self.observations(options.limit) {
            output.push_str("- ");
            output.push_str(&strip_markdown_code(&observation));
            output.push('\n');
        }
        output.push_str("\nTOP FRAMES\n");
        output.push_str(&self.render_frames(options, ReportFormat::Terminal, FrameOrder::Flat));
        output.push_str("\nTOP STACKS\n");
        output.push_str(&self.render_stacks(options, ReportFormat::Terminal));
        output
    }

    fn observations(&self, limit: usize) -> Vec<String> {
        if self.total_value == 0 {
            return vec!["The selected sample type has no measured cost.".to_owned()];
        }
        let mut observations = Vec::new();
        if let Some(frame) = self.top_frames.iter().find(|frame| frame.flat != 0) {
            observations.push(format!(
                "{} is the largest self-cost hotspot at {:.2}% ({}).",
                markdown_code(&frame.function),
                percent(frame.flat, self.total_value),
                self.format_value(frame.flat)
            ));
        }

        let mut cumulative: Vec<_> = self
            .top_frames
            .iter()
            .filter(|frame| frame.cumulative != 0)
            .collect();
        cumulative.sort_by(|a, b| {
            b.cumulative
                .saturating_abs()
                .cmp(&a.cumulative.saturating_abs())
        });
        let bottleneck = cumulative
            .iter()
            .copied()
            .find(|frame| frame.cumulative.saturating_abs() < self.total_value)
            .or_else(|| cumulative.first().copied());
        if let Some(frame) = bottleneck {
            observations.push(format!(
                "{} is on {:.2}% of cumulative cost ({}).",
                markdown_code(&frame.function),
                percent(frame.cumulative, self.total_value),
                self.format_value(frame.cumulative)
            ));
        }

        let stack_count = limit.clamp(1, 3).min(self.top_stacks.len());
        if stack_count > 0 {
            let value = self
                .top_stacks
                .iter()
                .take(stack_count)
                .fold(0i64, |total, stack| {
                    total.saturating_add(stack.value.saturating_abs())
                });
            observations.push(format!(
                "The top {stack_count} stack{} account{} for {:.2}% of measured cost.",
                if stack_count == 1 { "" } else { "s" },
                if stack_count == 1 { "s" } else { "" },
                percent(value, self.total_value)
            ));
        }
        observations
    }

    fn render_frames(
        &self,
        options: &RenderOptions,
        format: ReportFormat,
        order: FrameOrder,
    ) -> String {
        let mut frames: Vec<_> = self.top_frames.iter().collect();
        if order == FrameOrder::Cumulative {
            frames.sort_by(|a, b| {
                b.cumulative
                    .saturating_abs()
                    .cmp(&a.cumulative.saturating_abs())
                    .then_with(|| b.flat.saturating_abs().cmp(&a.flat.saturating_abs()))
                    .then_with(|| a.function.cmp(&b.function))
            });
        }
        frames.retain(|frame| {
            let value = match order {
                FrameOrder::Flat => frame.flat,
                FrameOrder::Cumulative => frame.cumulative,
            };
            percent(value, self.total_value) >= options.min_percent
        });
        frames.truncate(options.limit);

        match format {
            ReportFormat::Markdown => {
                let mut output = String::from(
                    "| Flat | Flat % | Cumulative | Cumulative % | Function | Source |\n\
                     | ---: | ---: | ---: | ---: | --- | --- |\n",
                );
                for frame in frames {
                    output.push_str(&format!(
                        "| {} | {:.2}% | {} | {:.2}% | {} | {} |\n",
                        self.format_value(frame.flat),
                        percent(frame.flat, self.total_value),
                        self.format_value(frame.cumulative),
                        percent(frame.cumulative, self.total_value),
                        markdown_code(&frame.function),
                        markdown_cell(&source_location(frame))
                    ));
                }
                output
            }
            ReportFormat::Terminal => {
                let mut output = String::from(
                    "HOT                       FLAT     FLAT%       CUM      CUM%  FUNCTION  SOURCE\n",
                );
                for frame in frames {
                    let hotness = match order {
                        FrameOrder::Flat => frame.flat,
                        FrameOrder::Cumulative => frame.cumulative,
                    };
                    output.push_str(&format!(
                        "{:<20} {:>9} {:>7.2}% {:>9} {:>7.2}%  {}  {}\n",
                        hotness_bar(percent(hotness, self.total_value), 20),
                        self.format_value(frame.flat),
                        percent(frame.flat, self.total_value),
                        self.format_value(frame.cumulative),
                        percent(frame.cumulative, self.total_value),
                        frame.function,
                        source_location(frame)
                    ));
                }
                output
            }
        }
    }

    fn render_stacks(&self, options: &RenderOptions, format: ReportFormat) -> String {
        let stacks: Vec<_> = self
            .top_stacks
            .iter()
            .filter(|stack| percent(stack.value, self.total_value) >= options.min_percent)
            .take(options.limit)
            .collect();
        match format {
            ReportFormat::Markdown => {
                let mut output = String::from("Stack direction: root caller → leaf sample.\n\n");
                for (index, stack) in stacks.into_iter().enumerate() {
                    output.push_str(&format!(
                        "### Stack {}\n\n\
                         - Cost: {}\n\
                         - Percent of total: {:.2}%\n\
                         - Frames: {}\n\
                         - Path: {}\n\n",
                        index + 1,
                        self.format_value(stack.value),
                        percent(stack.value, self.total_value),
                        stack.frames.len(),
                        stack
                            .frames
                            .iter()
                            .map(|frame| markdown_code(&frame.function))
                            .collect::<Vec<_>>()
                            .join(" → ")
                    ));
                }
                output
            }
            ReportFormat::Terminal => {
                let mut output = String::from("Stack direction: root caller -> leaf sample.\n\n");
                for (index, stack) in stacks.into_iter().enumerate() {
                    output.push_str(&format!(
                        "{:>2}. {:>9} {:>7.2}%  {}\n",
                        index + 1,
                        self.format_value(stack.value),
                        percent(stack.value, self.total_value),
                        stack
                            .frames
                            .iter()
                            .map(|frame| frame.function.as_str())
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ));
                }
                output
            }
        }
    }

    fn render_tree(&self, options: &RenderOptions, format: ReportFormat) -> String {
        let mut tree = CallTree::from_stacks(&self.top_stacks);
        tree.self_value = self.total_value.saturating_sub(tree.value.saturating_abs());
        let mut lines = vec![format!(
            "{} (100.00%) [self: {}]  all samples",
            self.format_value(self.total_value),
            self.format_value(tree.self_value)
        )];
        let eligible_nodes = tree.eligible_node_count(self, options.min_percent);
        let mut emitted = 0;
        tree.render_children(self, options, "", &mut lines, &mut emitted);
        let omitted_nodes = eligible_nodes.saturating_sub(emitted);
        if omitted_nodes > 0 {
            lines.push(format!(
                "… {omitted_nodes} additional call-tree node{} omitted by --limit",
                if omitted_nodes == 1 { "" } else { "s" }
            ));
        }
        let body = lines.join("\n");
        match format {
            ReportFormat::Terminal => format!(
                "Direction: callers at the top, callees beneath them.\n\
                 Row format: inclusive cost (percent of total) [self cost] function.\n\
                 At most {} call-tree nodes are shown, hottest branches first.\n\n\
                 {body}\n",
                options.limit
            ),
            ReportFormat::Markdown => {
                let mut output = format!(
                    "- Direction: callers at the top, callees beneath them.\n\
                     - Row format: `inclusive cost (percent of total) [self cost] function`.\n\
                     - Inclusive cost includes samples in the function and its descendants.\n\
                     - Self cost includes samples attributed directly to the function.\n\
                     - At most {} call-tree nodes are shown, hottest branches first.\n",
                    options.limit
                );
                if options.min_percent > 0.0 {
                    output.push_str(&format!(
                        "- Nodes below {:.2}% of total cost are omitted.\n",
                        options.min_percent
                    ));
                }
                output.push_str(&format!("\n```text\n{body}\n```\n"));
                output
            }
        }
    }

    fn format_value(&self, value: i64) -> String {
        format_value(value, &self.sample_unit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameOrder {
    Flat,
    Cumulative,
}

#[derive(Default)]
struct CallTree {
    frame: Option<ReportFrame>,
    value: i64,
    self_value: i64,
    children: HashMap<ReportFrameKey, CallTree>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReportFrameKey {
    function: String,
    file: String,
    line: i64,
}

impl From<&ReportFrame> for ReportFrameKey {
    fn from(frame: &ReportFrame) -> Self {
        Self {
            function: frame.function.clone(),
            file: frame.file.clone(),
            line: frame.line,
        }
    }
}

impl CallTree {
    fn from_stacks(stacks: &[StackStat]) -> Self {
        let mut root = Self::default();
        for stack in stacks {
            root.value = root.value.saturating_add(stack.value);
            let mut node = &mut root;
            for frame in &stack.frames {
                let key = ReportFrameKey::from(frame);
                node = node.children.entry(key).or_insert_with(|| Self {
                    frame: Some(frame.clone()),
                    ..Self::default()
                });
                node.value = node.value.saturating_add(stack.value);
            }
            node.self_value = node.self_value.saturating_add(stack.value);
        }
        root
    }

    fn eligible_node_count(&self, report: &ProfileReport, min_percent: f64) -> usize {
        self.children
            .values()
            .filter(|child| percent(child.value, report.total_value) >= min_percent)
            .map(|child| 1 + child.eligible_node_count(report, min_percent))
            .sum()
    }

    fn render_children(
        &self,
        report: &ProfileReport,
        options: &RenderOptions,
        prefix: &str,
        lines: &mut Vec<String>,
        emitted: &mut usize,
    ) {
        let mut children: Vec<_> = self.children.values().collect();
        children.retain(|child| percent(child.value, report.total_value) >= options.min_percent);
        children.sort_by(|a, b| {
            b.value
                .saturating_abs()
                .cmp(&a.value.saturating_abs())
                .then_with(|| {
                    a.frame
                        .as_ref()
                        .map(|frame| frame.function.as_str())
                        .cmp(&b.frame.as_ref().map(|frame| frame.function.as_str()))
                })
        });
        let child_count = children.len();
        for (index, child) in children.into_iter().enumerate() {
            if *emitted >= options.limit {
                break;
            }
            let is_last = index + 1 == child_count;
            let connector = if is_last { "└─ " } else { "├─ " };
            if let Some(frame) = &child.frame {
                lines.push(format!(
                    "{prefix}{connector}{} ({:.2}%) [self: {}]  {}",
                    report.format_value(child.value),
                    percent(child.value, report.total_value),
                    report.format_value(child.self_value),
                    frame.function
                ));
                *emitted += 1;
            }
            let descendant_prefix = format!("{prefix}{}", if is_last { "   " } else { "│  " });
            child.render_children(report, options, &descendant_prefix, lines, emitted);
        }
    }
}

fn percent(value: i64, total: i64) -> f64 {
    if total == 0 {
        0.0
    } else {
        value.saturating_abs() as f64 / total.saturating_abs() as f64 * 100.0
    }
}

fn hotness_bar(percentage: f64, width: usize) -> String {
    let filled = ((percentage.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!(
        "{}{}",
        "█".repeat(filled),
        " ".repeat(width.saturating_sub(filled))
    )
}

fn format_value(value: i64, unit: &str) -> String {
    let magnitude = value.saturating_abs();
    match unit.to_ascii_lowercase().as_str() {
        "nanoseconds" | "nanosecond" | "ns" if magnitude >= 1_000_000_000 => {
            format!("{:.2}s", value as f64 / 1_000_000_000.0)
        }
        "nanoseconds" | "nanosecond" | "ns" if magnitude >= 1_000_000 => {
            format!("{:.2}ms", value as f64 / 1_000_000.0)
        }
        "nanoseconds" | "nanosecond" | "ns" if magnitude >= 1_000 => {
            format!("{:.2}us", value as f64 / 1_000.0)
        }
        "nanoseconds" | "nanosecond" | "ns" => format!("{value}ns"),
        "bytes" | "byte" if magnitude >= 1_073_741_824 => {
            format!("{:.2}GiB", value as f64 / 1_073_741_824.0)
        }
        "bytes" | "byte" if magnitude >= 1_048_576 => {
            format!("{:.2}MiB", value as f64 / 1_048_576.0)
        }
        "bytes" | "byte" if magnitude >= 1_024 => {
            format!("{:.2}KiB", value as f64 / 1_024.0)
        }
        "bytes" | "byte" => format!("{value}B"),
        "" | "count" | "samples" | "objects" => value.to_string(),
        unit => format!("{value} {unit}"),
    }
}

fn source_location(frame: &FrameStat) -> String {
    match (frame.file.is_empty(), frame.line) {
        (true, _) => String::new(),
        (false, 0) => frame.file.clone(),
        (false, line) => format!("{}:{line}", frame.file),
    }
}

fn markdown_code(value: &str) -> String {
    if value.contains('`') {
        format!("`` {value} ``")
    } else {
        format!("`{value}`")
    }
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', " ")
}

fn strip_markdown_code(value: &str) -> String {
    value.replace('`', "")
}

fn decompress_if_needed(data: &[u8]) -> Result<Vec<u8>> {
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        GzDecoder::new(data)
            .read_to_end(&mut decoded)
            .context("decompressing gzip pprof profile")?;
        Ok(decoded)
    } else {
        Ok(data.to_vec())
    }
}

fn profile_string(profile: &PprofProfile, index: u64) -> &str {
    profile
        .strings
        .strings
        .get(index as usize)
        .map(String::as_str)
        .unwrap_or("")
}

fn sample_index(profile: &PprofProfile, requested: Option<&str>) -> Result<usize> {
    if profile.value_types.is_empty() {
        bail!("pprof profile has no sample types");
    }
    if let Some(requested) = requested {
        if let Ok(index) = requested.parse::<usize>() {
            if index < profile.value_types.len() {
                return Ok(index);
            }
            bail!(
                "sample type index {index} is outside the range 0..{}",
                profile.value_types.len()
            );
        }
        if let Some(index) = profile
            .value_types
            .iter()
            .position(|sample| profile_string(profile, sample.r#type) == requested)
        {
            return Ok(index);
        }
        let available = profile
            .value_types
            .iter()
            .map(|sample| profile_string(profile, sample.r#type))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("unknown sample type {requested:?}; available types: {available}");
    }

    if profile.default_sample_type != 0 {
        let wanted = profile_string(profile, profile.default_sample_type);
        if let Some(index) = profile
            .value_types
            .iter()
            .position(|sample| profile_string(profile, sample.r#type) == wanted)
        {
            return Ok(index);
        }
    }
    Ok(profile.value_types.len() - 1)
}

fn resolve_stack(
    profile: &PprofProfile,
    sample: &Sample,
    locations: &HashMap<u64, &Location>,
    functions: &HashMap<u64, &Function>,
) -> Vec<ResolvedFrame> {
    let mut frames = Vec::new();
    for location_id in sample.location_ids.iter().rev() {
        let Some(location) = locations.get(location_id) else {
            continue;
        };
        if location.lines.is_empty() {
            frames.push(ResolvedFrame {
                identity: FrameIdentity::Address(location.address),
                function: format!("0x{:016x}", location.address),
                file: String::new(),
                line: 0,
            });
            continue;
        }

        // Inline entries are leaf-first within a location, just like
        // locations are leaf-first within a sample.
        for line in location.lines.iter().rev() {
            let Some(function) = functions.get(&line.function_id) else {
                continue;
            };
            frames.push(ResolvedFrame {
                identity: FrameIdentity::Function(function.id),
                function: profile_string(profile, function.name).to_owned(),
                file: profile_string(profile, function.filename).to_owned(),
                line: line.line,
            });
        }
    }
    frames
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FrameIdentity {
    Function(u64),
    Address(u64),
}

#[derive(Debug)]
struct ResolvedFrame {
    identity: FrameIdentity,
    function: String,
    file: String,
    line: i64,
}

impl ResolvedFrame {
    fn stat(&self) -> FrameStat {
        FrameStat {
            function: self.function.clone(),
            file: self.file.clone(),
            line: self.line,
            flat: 0,
            cumulative: 0,
        }
    }

    fn report_frame(&self) -> ReportFrame {
        ReportFrame {
            function: self.function.clone(),
            file: self.file.clone(),
            line: self.line,
        }
    }
}
