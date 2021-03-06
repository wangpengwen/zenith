/**
 * Copyright 2019 Benjamin Vaisvil
 */
use crate::metrics::*;
use crate::util::*;
use crate::zprocess::*;
use byte_unit::{Byte, ByteUnit};
use chrono::prelude::DateTime;
use chrono::{Local, Datelike, Timelike};
use std::borrow::Cow;
use std::fmt::Debug;
use std::io;
use std::io::Stdout;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use sysinfo::DiskExt;
use termion::event::Key;
use termion::input::MouseTerminal;
use termion::raw::{IntoRawMode, RawTerminal};
use termion::screen::AlternateScreen;
use tui::backend::{TermionBackend, Backend};

use tui::layout::{Constraint, Direction, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{BarChart, Block, Borders, List, Paragraph, Row, Sparkline, Table, Text, Widget, Dataset};
use tui::Frame;
use tui::Terminal;
use sled;
use std::ops::Mul;

type ZBackend = TermionBackend<AlternateScreen<MouseTerminal<RawTerminal<Stdout>>>>;

macro_rules! float_to_byte_string{
    ($x:expr, $unit:expr) =>{
        match Byte::from_unit($x, $unit){
            Ok(b) => b.get_appropriate_unit(false).to_string().replace(" ", ""),
            Err(_) => String::from("")
        }
    };
}

macro_rules! set_section_height{
    ($x:expr, $val:expr) => {
        if $x + $val > 0{
            $x += $val;
        }
    };
}



#[derive(FromPrimitive, PartialEq, Copy, Clone)]
enum Section{
    CPU = 0,
    Network = 1,
    Disk = 2,
    Process = 3
}

fn mem_title(app: &CPUTimeApp) -> String {
    let mut mem: u64 = 0;
    if app.mem_utilization > 0 && app.mem_total > 0 {
        mem = ((app.mem_utilization as f32 / app.mem_total as f32) * 100.0) as u64;
    }
    let mut swp: u64 = 0;
    if app.swap_utilization > 0 && app.swap_total > 0 {
        swp = ((app.swap_utilization as f32 / app.swap_total as f32) * 100.0) as u64;
    }

    format!(
        "MEM [{}] Usage [{: >3}%] SWP [{}] Usage [{: >3}%]",
        float_to_byte_string!(app.mem_total as f64, ByteUnit::KB),
        mem,
        float_to_byte_string!(app.swap_total as f64, ByteUnit::KB),
        swp
    )
}

fn cpu_title(app: &CPUTimeApp) -> String {
    let top_process_name = match &app.cum_cpu_process {
        Some(p) => p.name.as_str(),
        None => "",
    };
    let top_process_amt = match &app.cum_cpu_process {
        Some(p) => p.user_name.as_str(),
        None => "",
    };
    let top_pid = match &app.cum_cpu_process{
        Some(p) => p.pid,
        None => 0
    };
    let h = match app.histogram_map.get("cpu_usage_histogram") {
        Some(h) => &h.data,
        None => return String::from(""),
    };
    let mean: f64 = match h.len() {
        0 => 0.0,
        _ => h.iter().sum::<u64>() as f64 / h.len() as f64,
    };
    format!(
        "CPU [{: >3}%] MEAN [{: >3.2}%] TOP [{} - {} - {}]",
        app.cpu_utilization, mean, top_pid, top_process_name, top_process_amt
    )
}

fn render_process_table<'a>(
    app: &CPUTimeApp,
    width: u16,
    area: Rect,
    process_table_start: usize,
    f: &mut Frame<ZBackend>,
    selected_section: &Section
) {
    if area.height < 5 {
        return; // not enough space to draw anything
    }
    let style = match selected_section {
        Section::Process => Style::default().fg(Color::Red),
        _ => Style::default()
    };
    let display_height = area.height as usize - 4; // 4 for the margins and table header

    let end = process_table_start + display_height;

    let rows: Vec<Vec<String>> = app
        .processes
        .iter()
        .map(|pid| {
            let p = app.process_map.get(pid).expect("Pid present in processes but not in map.");
            vec![
                format!("{: >5}", p.pid),
                format!("{: <10}", p.user_name),
                format!("{: <3}", p.priority),
                format!("{:>5.1}", p.cpu_usage),
                format!("{:>5.1}", (p.memory as f64 / app.mem_total as f64) * 100.0),
                format!("{:>8}", float_to_byte_string!(p.memory as f64, ByteUnit::KB).replace("B", "")),
                format!("{: >8}", float_to_byte_string!(p.virtual_memory as f64, ByteUnit::KB).replace("B", "")),
                format!("{:1}", p.status.to_single_char()),
                format!("{:>8}", float_to_byte_string!(p.get_read_bytes_sec(), ByteUnit::B).replace("B", "")),
                format!("{:>8}", float_to_byte_string!(p.get_write_bytes_sec(), ByteUnit::B).replace("B", "")),
                format!("{} - ", p.name) + &[p.exe.as_str(), p.command.join(" ").as_str()].join(" "),
            ]
        })
        .collect();

    let mut header = vec![
        String::from("PID   "),
        String::from("USER       "),
        String::from("P   "),
        String::from("CPU%  "),
        String::from("MEM%  "),
        String::from("MEM     "),
        String::from("VIRT     "),
        String::from("S "),
        String::from("READ/s   "),
        String::from("WRITE/s  "),
    ];
    //figure column widths
    let mut widths: Vec<u16> = header.iter().map(|item| item.len() as u16).collect();
    let s: u16 = widths.iter().sum();
    let mut cmd_width = width as i16 - s as i16 - 3;
    if cmd_width < 0 {
        cmd_width = 0;
    }
    let cmd_width = cmd_width as u16;
    let mut cmd_header = String::from("CMD");
    for _i in 3..cmd_width {
        cmd_header.push(' ');
    }
    header.push(cmd_header);
    widths.push(header.last().unwrap().len() as u16);
    header[app.psortby as usize].pop();
    let sort_ind = match app.psortorder {
        ProcessTableSortOrder::Ascending => '↑',
        ProcessTableSortOrder::Descending => '↓',
    };
    header[app.psortby as usize].insert(0, sort_ind); //sort column indicator
    let rows = rows.iter().enumerate().filter_map(|(i, r)| {
        if i >= process_table_start && i < end {
            if app.highlighted_row == i {
                Some(Row::StyledData(
                    r.into_iter(),
                    Style::default().fg(Color::Magenta).modifier(Modifier::BOLD),
                ))
            } else {
                Some(Row::Data(r.into_iter()))
            }
        } else {
            None
        }
    });

    Table::new(header.into_iter(), rows)
        .block(
            Block::default().borders(Borders::ALL).border_style(style)
                .title(
                format!(
                    "Tasks [{}] Threads [{}]  Navigate [↑/↓] Sort Col [,/.] Asc/Dec [/]",
                    app.processes.len(),
                    app.threads_total
                )
                .as_str(),
            ).title_style(style),
        )
        .widths(widths.as_slice())
        .column_spacing(0)
        .header_style(Style::default().bg(Color::DarkGray))
        .render(f, area);
}

fn render_cpu_histogram(app: &CPUTimeApp, area: Rect, f: &mut Frame<ZBackend>, zf: &u32, offset: &usize) {
    let title = cpu_title(&app);
    let h = match app
        .histogram_map
        .get_zoomed("cpu_usage_histogram", *zf, area.width as usize, *offset)
    {
        Some(h) => h.data,
        None => return,
    };
    Sparkline::default()
        .block(Block::default().title(title.as_str()))
        .data(&h)
        .style(Style::default().fg(Color::Blue))
        .max(100)
        .render(f, area);
}

fn render_memory_histogram(app: &CPUTimeApp, area: Rect, f: &mut Frame<ZBackend>, zf: &u32, offset: &usize) {
    let h = match app
        .histogram_map
        .get_zoomed("mem_utilization", *zf, area.width as usize, *offset)
    {
        Some(h) => h.data,
        None => return,
    };
    let title2 = mem_title(&app);
    Sparkline::default()
        .block(Block::default().title(title2.as_str()))
        .data(&h)
        .style(Style::default().fg(Color::Cyan))
        .max(100)
        .render(f, area);
}

fn render_cpu_bars(app: &CPUTimeApp, area: Rect, width: u16, f: &mut Frame<ZBackend>, style: &Style) {
    let mut cpus = app.cpus.to_owned();
    let mut bars: Vec<(&str, u64)> = vec![];
    let mut bar_gap: u16 = 1;

    let mut np = cpus.len() as u16;
    if np == 0 {
        np = 1;
    }

    let mut constraints: Vec<Constraint> = vec![];
    let mut half: usize = np as usize;
    if np > width - 2 {
        constraints.push(Constraint::Percentage(50));
        constraints.push(Constraint::Percentage(50));
        half = np as usize / 2;
    } else {
        constraints.push(Constraint::Percentage(100));
    }
    //compute bar width and gutter/gap

    if width > 2 && (half * 2) >= (width - 2) as usize {
        bar_gap = 0;
    }
    let width = width - 2;
    let mut cpu_bw = ((width as f32 - (half as u16 * bar_gap) as f32) / half as f32) as i16;
    if cpu_bw < 1 {
        cpu_bw = 1;
    }
    let cpu_bw = cpu_bw as u16;
    for (i, (p, u)) in cpus.iter_mut().enumerate() {
        if i > 8 && cpu_bw == 1 {
            p.remove(0);
        }
        bars.push((p.as_str(), u.clone()));
    }

    Block::default()
        .title(format!("CPU(S) {}@{} MHz", np, app.frequency).as_str())
        .borders(Borders::ALL).border_style(*style).title_style(*style)
        .render(f, area);
    let cpu_bar_layout = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints(constraints.as_ref())
        .split(area);

    if np > width {
        BarChart::default()
            .data(&bars[0..half])
            .bar_width(cpu_bw)
            .bar_gap(bar_gap)
            .max(100)
            .style(Style::default().fg(Color::Green))
            .value_style(Style::default().bg(Color::Green).modifier(Modifier::BOLD))
            .render(f, cpu_bar_layout[0]);
        BarChart::default()
            .data(&bars[half..])
            .bar_width(cpu_bw)
            .bar_gap(bar_gap)
            .max(100)
            .style(Style::default().fg(Color::Green))
            .value_style(Style::default().bg(Color::Green).modifier(Modifier::BOLD))
            .render(f, cpu_bar_layout[1]);
    } else {
        BarChart::default()
            .data(bars.as_slice())
            .bar_width(cpu_bw)
            .bar_gap(bar_gap)
            .max(100)
            .style(Style::default().fg(Color::Green))
            .value_style(Style::default().bg(Color::Green).modifier(Modifier::BOLD))
            .render(f, cpu_bar_layout[0]);
    }
}

fn render_net(app: &CPUTimeApp, area: Rect, f: &mut Frame<ZBackend>, zf: &u32, offset: &usize, selected_section: &Section) {
    let style = match selected_section {
        Section::Network => Style::default().fg(Color::Red),
        _ => Style::default()
    };
    Block::default()
    .title("Network")
    .borders(Borders::ALL).border_style(style)
    .render(f, area);
    let network_layout = Layout::default()
        .margin(0)
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(10)].as_ref())
        .split(area);
    let net = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(network_layout[1]);

    let net_up = float_to_byte_string!(app.net_out as f64, ByteUnit::B);
    let h_out = match app
        .histogram_map
        .get_zoomed("net_out", *zf, net[0].width as usize, *offset)
    {
        Some(h) => h.data,
        None => return,
    };

    let up_max: u64 = match h_out.iter().max() {
        Some(x) => x.clone(),
        None => 1,
    };
    let up_max_bytes = float_to_byte_string!(up_max as f64, ByteUnit::B);

    Sparkline::default()
        .block(
            Block::default().title(
                format!(
                    "↑ [{:^10}] Max [{:^10}]",
                    net_up.to_string(),
                    up_max_bytes
                )
                .as_str(),
            ),
        )
        .data(&h_out)
        .style(Style::default().fg(Color::LightYellow))
        .max(up_max)
        .render(f, net[0]);

    let net_down = float_to_byte_string!(app.net_in as f64, ByteUnit::B);
    let h_in = match app
        .histogram_map
        .get_zoomed("net_in", *zf, net[1].width as usize, *offset)
    {
        Some(h) => h.data,
        None => return,
    };

    let down_max: u64 = match h_in.iter().max() {
        Some(x) => x.clone(),
        None => 1,
    };
    let down_max_bytes = float_to_byte_string!(down_max as f64, ByteUnit::B);
    Sparkline::default()
        .block(
            Block::default().title(
                format!(
                    "↓ [{:^10}] Max [{:^10}]",
                    net_down,
                    down_max_bytes
                )
                .as_str(),
            ),
        )
        .data(&h_in)
        .style(Style::default().fg(Color::LightMagenta))
        .max(down_max)
        .render(f, net[1]);

    let ips = app.network_interfaces.iter().map(|n| {
        Text::Styled(
            Cow::Owned(format!("{:<8.8} : {}", n.name, n.ip)),
            Style::default().fg(Color::Green),
        )
    });
    List::new(ips)
        .block(Block::default().title("Network").borders(Borders::ALL).border_style(style).title_style(style))
        .render(f, network_layout[0]);
}

fn render_process(app: &CPUTimeApp, layout: Rect, f: &mut Frame<ZBackend>, width: u16, selected_section: &Section, process_message: &Option<String>) {
    let style = match selected_section {
        Section::Process => Style::default().fg(Color::Red),
        _ => Style::default()
    };
    match &app.selected_process {
        Some(p) => {
            Block::default()
                .title(format!("Process: {0}", p.name).as_str())
                .borders(Borders::ALL).border_style(style).title_style(style)
                .render(f, layout);
            let v_sections = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Length(2), Constraint::Min(1)].as_ref())
                .split(layout);
            let h_sections = Layout::default()
                .direction(Direction::Horizontal)
                .margin(0)
                .constraints(
                    [
                        Constraint::Percentage(50),
                        Constraint::Length(1),
                        Constraint::Percentage(50),
                    ]
                    .as_ref(),
                )
                .split(v_sections[1]);
            
            Block::default()
                .title(format!("(b)ack (s)uspend (r)esume (k)ill [SIGKILL] (t)erminate [SIGTERM] {:} {: >width$}", process_message.as_ref().unwrap_or(&String::from("")), "", width = layout.width as usize).as_str())
                .title_style(Style::default().bg(Color::DarkGray).fg(Color::White)).render(f, v_sections[0]);

            //Block::default().borders(Borders::LEFT).render(f, h_sections[1]);

            let alive = if p.end_time.is_some() {
                format!(
                    "dead since {:}",
                    DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(p.end_time.unwrap()))
                )
            } else {
                format!("alive")
            };
            let start_time =
                DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(p.start_time));
            let et = match p.end_time {
                Some(t) => DateTime::<Local>::from(UNIX_EPOCH + Duration::from_secs(t)),
                None => Local::now(),
            };
            let d = et - start_time;
            let d = format!(
                "{:0>2}:{:0>2}:{:0>2}",
                d.num_hours(),
                d.num_minutes() % 60,
                d.num_seconds() % 60
            );

            let rhs_style = Style::default().fg(Color::Green);
            let text = vec![
                Text::raw("Name:                  "),
                Text::styled(format!("{:} ({:})", &p.name, alive), rhs_style),
                Text::raw("\n"),
                Text::raw("PID:                   "),
                Text::styled(format!("{:>7}", &p.pid), rhs_style),
                Text::raw("\n"),
                Text::raw("Command:               "),
                Text::styled(p.command.join(" "), rhs_style),
                Text::raw("\n"),
                Text::raw("User:                  "),
                Text::styled(&p.user_name, rhs_style),
                Text::raw("\n"),
                Text::raw("Start Time:            "),
                Text::styled(format!("{:}", start_time), rhs_style),
                Text::raw("\n"),
                Text::raw("Total Run Time:        "),
                Text::styled(format!("{:}", d), rhs_style),
                Text::raw("\n"),
                Text::raw("CPU Usage:             "),
                Text::styled(format!("{:>7.2} %", &p.cpu_usage), rhs_style),
                Text::raw("\n"),
                Text::raw("Threads:               "),
                Text::styled(format!("{:>7}", &p.threads_total), rhs_style),
                Text::raw("\n"),
                Text::raw("Status:                "),
                Text::styled(format!("{:}", p.status), rhs_style),
                Text::raw("\n"),
                Text::raw("Priority:              "),
                Text::styled(format!("{:>7}", p.priority), rhs_style),
                Text::raw("\n"),
                Text::raw("MEM Usage:             "),
                Text::styled(
                    format!(
                        "{:>7.2} %",
                        (p.memory as f64 / app.mem_total as f64) * 100.0
                    ),
                    rhs_style,
                ),
                Text::raw("\n"),
                Text::raw("Total Memory:          "),
                Text::styled(
                    format!(
                        "{:>10}",
                        float_to_byte_string!(p.memory as f64, ByteUnit::KB)
                    ),
                    rhs_style,
                ),
                Text::raw("\n"),
                Text::raw("Disk Read:             "),
                Text::styled(
                    format!(
                        "{:>10} {:}/s",
                        float_to_byte_string!(p.read_bytes as f64, ByteUnit::B),
                        float_to_byte_string!(p.get_read_bytes_sec(), ByteUnit::B)
                    ),
                    rhs_style,
                ),
                Text::raw("\n"),
                Text::raw("Disk Write:            "),
                Text::styled(
                    format!(
                        "{:>10} {:}/s",
                        float_to_byte_string!(p.write_bytes as f64, ByteUnit::B),
                        float_to_byte_string!(p.get_write_bytes_sec(), ByteUnit::B)
                    ),
                    rhs_style,
                ),
                Text::raw("\n"),
            ];

            if text.len() > h_sections[0].height as usize * 3 {
                Paragraph::new(text[0..h_sections[0].height as usize * 3].iter())
                    .block(Block::default())
                    .wrap(false)
                    .render(f, h_sections[0]);

                Paragraph::new(text[h_sections[0].height as usize * 3..].iter())
                    .block(Block::default())
                    .wrap(false)
                    .render(f, h_sections[2]);
            } else {
                Paragraph::new(text.iter())
                    .block(Block::default())
                    .wrap(false)
                    .render(f, h_sections[0]);
            }
        }
        None => return,
    };
}

fn render_sensors(app: &CPUTimeApp, layout: Vec<Rect>, f: &mut Frame<ZBackend>, zf: &u32){
    let sensors = app.sensors.iter().map(|s|{
        Text::raw(format!("{:}:{:}", s.name, s.current_temp))
    });
    List::new(sensors)
        .block(Block::default().title("Sensors").borders(Borders::ALL))
        .render(f, layout[0]);
}

fn render_disk(app: &CPUTimeApp, layout: Rect, f: &mut Frame<ZBackend>, zf: &u32, offset: &usize, selected_section: &Section) {
    let style = match selected_section {
        Section::Disk => Style::default().fg(Color::Red),
        _ => Style::default()
    };
    Block::default().title("Disk").borders(Borders::ALL).border_style(style).render(f, layout);
    let disk_layout = Layout::default()
                        .margin(0)
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Length(30), Constraint::Min(10)].as_ref())
                        .split(layout);
    let area = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
        .split(disk_layout[1]);

    let read_up = float_to_byte_string!(app.disk_read as f64, ByteUnit::B);
    let h_read = match app
        .histogram_map
        .get_zoomed("disk_read", *zf, area[0].width as usize, *offset)
    {
        Some(h) => h.data,
        None => return,
    };

    let read_max: u64 = match h_read.iter().max() {
        Some(x) => x.clone(),
        None => 1,
    };
    let read_max_bytes = float_to_byte_string!(read_max as f64, ByteUnit::B);
    Sparkline::default()
        .block(
            Block::default().title(
                format!(
                    "R [{:^10}] Max [{:^10}]",
                    read_up,
                    read_max_bytes
                )
                .as_str(),
            ),
        )
        .data(&h_read)
        .style(Style::default().fg(Color::LightYellow))
        .max(read_max)
        .render(f, area[0]);

    let write_down = float_to_byte_string!(app.disk_write as f64, ByteUnit::B);
    let h_write = match app
        .histogram_map
        .get_zoomed("disk_write", *zf, area[1].width as usize, *offset)
    {
        Some(h) => h.data,
        None => return,
    };

    let write_max: u64 = match h_write.iter().max() {
        Some(x) => x.clone(),
        None => 1,
    };
    let write_max_bytes = float_to_byte_string!(write_max as f64, ByteUnit::B);
    Sparkline::default()
        .block(
            Block::default().title(
                format!(
                    "W [{:^10}] Max [{:^10}]",
                    write_down,
                    write_max_bytes
                )
                .as_str(),
            ),
        )
        .data(&h_write)
        .style(Style::default().fg(Color::LightMagenta))
        .max(write_max)
        .render(f, area[1]);
    let disks = app.disks.iter().map(|d| {
        if d.get_perc_free_space() < 10.0 {
            Text::Styled(
                Cow::Owned(format!(
                    "{:.2}%(!): {}",
                    d.get_perc_free_space(),
                    d.get_mount_point().display()
                )),
                Style::default().fg(Color::Red).modifier(Modifier::BOLD),
            )
        } else {
            Text::Styled(
                Cow::Owned(format!(
                    "{:.2}%: {}",
                    d.get_perc_free_space(),
                    d.get_mount_point().display()
                )),
                Style::default().fg(Color::Green),
            )
        }
    });
    List::new(disks)
        .block(Block::default().title("Disks").borders(Borders::ALL).border_style(style).title_style(style))
        .render(f, disk_layout[0]);
}

fn display_time(start: DateTime<Local>, end: DateTime<Local>) -> String {
    if start.day() == end.day() && start.month() == end.month() {
        return format!(" ({:02}:{:02}:{:02} - {:02}:{:02}:{:02})",
                       start.hour(), start.minute(), start.second(),
                       end.hour(), end.minute(), end.second()
        );
    }
    format!(" ({:} {:02}:{:02}:{:02} - {:} {:02}:{:02}:{:02})",
            start.date(), start.hour(), start.minute(), start.second(),
            end.date(), end.hour(), end.minute(), end.second() )
}

fn render_top_title_bar(app: &CPUTimeApp, area: Rect, f: &mut Frame<ZBackend>, zf: &u32, offset: &usize, tick: &Duration) {
    let hist_duration = app.histogram_map.hist_duration(area.width as usize, *zf);
    let offset_duration = chrono::Duration::from_std(tick.mul(*offset as u32).mul(*zf)).expect("Couldn't convert from std");
    let now = Local::now();
    let start = now.checked_sub_signed(hist_duration + offset_duration).expect("Couldn't compute time");
    let end = now.checked_sub_signed(offset_duration).expect("Couldn't add time");
    let default_style = Style::default().bg(Color::DarkGray).fg(Color::White);
    let back_in_time = if offset_duration.num_seconds() > 0{
        format!("(-{:02}:{:02}:{:02})", 
        offset_duration.num_hours(), 
        offset_duration.num_minutes() % 60, 
        offset_duration.num_seconds() % 60)
    }
    else
    {
        String::from("")
    };
    let line = vec![
        Text::styled(
            format!(" {:}", app.hostname),
            default_style.modifier(Modifier::BOLD),
        ),
        Text::styled(format!(" [{:} {:}]", app.osname, app.release), default_style),
        Text::styled("[Showing: ", default_style),
        Text::styled(
            format!("{:} mins", hist_duration.num_minutes()),
            default_style.fg(Color::Green),
        ),
        Text::styled(display_time(start, end), default_style),
        Text::styled(back_in_time, default_style.modifier(Modifier::BOLD)),
        Text::styled("]", default_style),
        Text::styled(" <tab> sections, (e)pand (m)inimize [+/-] zoom [←/→] scroll histogram (`) reset", default_style),
        Text::styled(" (q)uit", default_style),
        Text::styled(
            format!("{: >width$}", "", width = area.width as usize),
            default_style,
        ),
    ];
    Paragraph::new(line.iter()).render(f, area);
}

fn render_cpu(app: &CPUTimeApp, area: Rect, f: &mut Frame<ZBackend>, zf: &u32, offset: &usize, selected_section: &Section){
    let style = match selected_section {
        Section::CPU => Style::default().fg(Color::Red),
        _ => Style::default()
    };
    Block::default()
    .title("")
    .borders(Borders::ALL).border_style(style)
    .render(f, area);
    let cpu_layout = Layout::default()
        .margin(0)
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(30), Constraint::Min(10)].as_ref())
        .split(area);

    let cpu_mem = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints(
            [Constraint::Percentage(50), Constraint::Percentage(50)].as_ref(),
        )
        .split(cpu_layout[1]);
    render_cpu_histogram(&app, cpu_mem[0], f, zf, offset);
    render_memory_histogram(&app, cpu_mem[1], f, zf, offset);
    render_cpu_bars(&app, cpu_layout[0], 30, f, &style);
}

pub struct TerminalRenderer {
    terminal: Terminal<TermionBackend<AlternateScreen<MouseTerminal<RawTerminal<Stdout>>>>>,
    app: CPUTimeApp,
    events: Events,
    process_table_row_start: usize,
    cpu_height: i16,
    net_height: i16,
    disk_height: i16,
    process_height: i16,
    sensor_height: i16,
    zoom_factor: u32,
    hist_start_offset: usize,
    selected_section: Section,
    constraints: Vec<Constraint>,
    process_message: Option<String>
}

impl<'a> TerminalRenderer {
    pub fn new(
        tick_rate: u64,
        cpu_height: i16,
        net_height: i16,
        disk_height: i16,
        process_height: i16,
        sensor_height: i16,
        db: Option<sled::Db>
    ) -> TerminalRenderer {
        let stdout = io::stdout()
            .into_raw_mode()
            .expect("Could not bind to STDOUT in raw mode.");
        let stdout = MouseTerminal::from(stdout);
        let stdout = AlternateScreen::from(stdout);
        let backend = TermionBackend::new(stdout);
        let mut constraints = vec![
            Constraint::Length(1),
            Constraint::Length(cpu_height as u16),
            Constraint::Length(net_height as u16),
            Constraint::Length(disk_height as u16),
            //Constraint::Length(*sensor_height),
        ];
        if process_height > 0 {
            constraints.push(Constraint::Min(process_height as u16));
        }
        TerminalRenderer {
            terminal: Terminal::new(backend).expect("Couldn't create new terminal with backend"),
            app: CPUTimeApp::new(Duration::from_millis(tick_rate), db),
            events: Events::new(tick_rate),
            process_table_row_start: 0,
            cpu_height,
            net_height,
            disk_height,
            process_height,
            sensor_height,
            zoom_factor: 1,
            selected_section: Section::Process,
            constraints,
            process_message: None,
            hist_start_offset: 0
        }
    }


    async fn set_constraints(&mut self){
        let mut constraints = vec![
            Constraint::Length(1),
            Constraint::Length(self.cpu_height as u16),
            Constraint::Length(self.net_height as u16),
            Constraint::Length(self.disk_height as u16),
            //Constraint::Length(*sensor_height),
        ];
        if self.process_height > 0 {
            constraints.push(Constraint::Min(self.process_height as u16));
        }
        self.constraints = constraints;
    }

    async fn set_section_height(&mut self, val: i16){
        match self.selected_section{
            Section::CPU => set_section_height!(self.cpu_height, val),
            Section::Disk => set_section_height!(self.disk_height, val),
            Section::Network => set_section_height!(self.net_height, val),
            Section::Process => set_section_height!(self.process_height, val),
        }
        self.set_constraints().await;
    }

    pub async fn start(&mut self) {
        
        loop {
            let app = &self.app;
            let pst = &self.process_table_row_start;
            let process_height = &self.process_height;
            let mut width: u16 = 0;
            let mut process_table_height: u16 = 0;
            let zf = &self.zoom_factor;
            let constraints = &self.constraints;
            let selected = &self.selected_section;
            let process_message = &self.process_message;
            let offset = &self.hist_start_offset;
            let tick = &self.app.histogram_map.tick;

            self.terminal
                .draw(|mut f| {
                    width = f.size().width;
                    // create layouts
                    // primary vertical
                    let v_sections = Layout::default()
                        .direction(Direction::Vertical)
                        .margin(0)
                        .constraints(constraints.as_ref())
                        .split(f.size());

                    render_top_title_bar(app, v_sections[0], &mut f, zf, offset, tick);
                    render_cpu(app, v_sections[1], &mut f, zf, offset, selected);
                    render_net(&app, v_sections[2], &mut f, zf, offset, selected);
                    render_disk(&app, v_sections[3], &mut f, zf, offset, selected);
                    if *process_height > 0{
                        if let Some(area) = v_sections.last(){
                            if app.selected_process.is_none() {
                                render_process_table(&app, width, *area, *pst, &mut f, selected);
                                if area.height > 4 {
                                    // account for table border & margins.
                                    process_table_height = area.height - 5;
                                }
                            } else if app.selected_process.is_some() {
                                render_process(&app, *area, &mut f, width, selected, process_message);
                            }
                        }
                    }
                    
                    
                    //render_sensors(&app, sensor_layout, &mut f, zf);
                }).expect("Could not draw frame.");

            match self.events.next().expect("No new event.") {
                Event::Input(input) => {
                    if input == Key::Char('q') {
                        break;
                    } else if input == Key::Up {
                        if self.app.selected_process.is_some() {} else {
                            self.app.highlight_up();
                            if self.process_table_row_start > 0
                                && self.app.highlighted_row < self.process_table_row_start
                            {
                                self.process_table_row_start -= 1;
                            }
                        }
                    } else if input == Key::Down {
                        if self.app.selected_process.is_some() {} else {
                            self.app.highlight_down();
                            if self.process_table_row_start < self.app.process_map.len()
                                && self.app.highlighted_row
                                > (self.process_table_row_start + process_table_height as usize)
                            {
                                self.process_table_row_start += 1;
                            }
                        }
                    } else if input == Key::Left {
                        match self.app.histogram_map.histograms_width(){
                            Some(w) => {
                                self.hist_start_offset += 1;
                                if self.hist_start_offset > w + 1{
                                    self.hist_start_offset = w - 1;
                                }
                            },
                            None => {}
                        }
                        self.hist_start_offset += 1;
                    } else if input == Key::Right {
                        if self.hist_start_offset > 0{
                            self.hist_start_offset -= 1;
                        }
                    } else if input == Key::Char('.') || input == Key::Char('>') {
                        if self.app.psortby == ProcessTableSortBy::Cmd {
                            self.app.psortby = ProcessTableSortBy::Pid;
                        } else {
                            self.app.psortby =
                                num::FromPrimitive::from_u32(self.app.psortby as u32 + 1).expect("invalid value to set psortby");
                        }
                        self.app.sort_process_table();
                    } else if input == Key::Char(',') || input == Key::Char('<') {
                        if self.app.psortby == ProcessTableSortBy::Pid {
                            self.app.psortby = ProcessTableSortBy::Cmd;
                        } else {
                            self.app.psortby =
                                num::FromPrimitive::from_u32(self.app.psortby as u32 - 1).expect("invalid value to set psortby");
                        }
                        self.app.sort_process_table();
                    } else if input == Key::Char('/') {
                        match self.app.psortorder {
                            ProcessTableSortOrder::Ascending => {
                                self.app.psortorder = ProcessTableSortOrder::Descending
                            }
                            ProcessTableSortOrder::Descending => {
                                self.app.psortorder = ProcessTableSortOrder::Ascending
                            }
                        }
                        self.app.sort_process_table();
                    } else if input == Key::Char('+') || input == Key::Char('=') {
                        if self.zoom_factor > 1 {
                            self.zoom_factor -= 1;
                        }
                    } else if input == Key::Char('-') {
                        if self.zoom_factor < 100 {
                            self.zoom_factor += 1;
                        }
                    } else if input == Key::Char('\n') {
                        self.app.select_process();
                        self.process_message = None;
                    } else if input == Key::Esc || input == Key::Char('b') {
                        self.app.selected_process = None;
                        self.process_message = None;
                    } else if input == Key::Char('s') {
                        self.process_message = None;
                        self.process_message = match &self.app.selected_process {
                            Some(p) => Some(p.suspend().await),
                            None => None,
                        };
                    } else if input == Key::Char('r') {
                        self.process_message = None;
                        self.process_message = match &self.app.selected_process {
                            Some(p) => Some(p.resume().await),
                            None => None,
                        };
                    } else if input == Key::Char('k') {
                        self.process_message = None;
                        self.process_message = match &self.app.selected_process {
                            Some(p) => Some(p.kill().await),
                            None => None,
                        };
                    } else if input == Key::Char('t') {
                        self.process_message = None;
                        self.process_message = match &self.app.selected_process {
                            Some(p) => Some(p.terminate().await),
                            None => None,
                        };
                    }
                    else if input == Key::Char('\t'){
                        let mut i = self.selected_section as u32 + 1;
                        if i > 3{
                            i = 0;
                        }
                        self.selected_section = num::FromPrimitive::from_u32(i).unwrap_or(Section::CPU);
                    }
                    else if input == Key::Char('m'){
                        self.set_section_height(-2).await;
                    }
                    else if input == Key::Char('e'){
                        self.set_section_height(2).await;
                    }
                    else if input == Key::Char('`'){
                        self.zoom_factor = 1;
                        self.hist_start_offset = 0;
                    }
                    else if input == Key::Ctrl('c'){
                        break;
                    }
                }
                Event::Tick => {
                    self.app.update(width).await;
                }
                Event::Save => {
                    self.app.save_state().await;
                }
            }
        }
    }
}
