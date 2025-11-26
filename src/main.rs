use clap::Parser;
use std::collections::HashMap;
use serde::Deserialize;
use regex::Regex;
use lazy_static::lazy_static;

const CSV_DATA: &str = include_str!("../data/equipmentInfo.csv");

fn deserialize_f64_custom<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt_s: Result<Option<String>, _> = Deserialize::deserialize(deserializer);
    match opt_s {
        Ok(Some(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() { return Ok(None); }
            match trimmed.parse::<f64>() {
                Ok(v) => if v > -90000.0 { Ok(Some(v)) } else { Ok(None) },
                Err(_) => Ok(None),
            }
        },
        Ok(None) => Ok(None),
        Err(_) => Ok(None),
    }
}

fn deserialize_ahri<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt_v: Result<Option<f64>, _> = Deserialize::deserialize(deserializer);
    match opt_v {
        Ok(Some(v)) => if v > 0.0 { Ok(Some(v as u64)) } else { Ok(None) },
        _ => Ok(None),
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct MachineData {
    #[serde(rename = "model number")]
    pub model_number: String,
    
    #[serde(rename = "machine code")]
    pub machine_code: Option<String>,
    
    #[serde(rename = "AHRI", deserialize_with = "deserialize_ahri")]
    pub ahri: Option<u64>,

    #[serde(rename = "Btu@95min", deserialize_with = "deserialize_f64_custom")]
    pub btu_95_min: Option<f64>,

    // Heating points for interpolation
    
    #[serde(rename = "Btu@lowest max", deserialize_with = "deserialize_f64_custom")]
    pub btu_lowest_max: Option<f64>,

    #[serde(rename = "lowest temperature", deserialize_with = "deserialize_f64_custom")]
    pub lowest_temp: Option<f64>,

    #[serde(rename = "Btu@5max", deserialize_with = "deserialize_f64_custom")]
    pub btu_5_max: Option<f64>,

    #[serde(rename = "Btu@17max", deserialize_with = "deserialize_f64_custom")]
    pub btu_17_max: Option<f64>,

    #[serde(rename = "Btu@17rated", deserialize_with = "deserialize_f64_custom")]
    pub btu_17_rated: Option<f64>,

    #[serde(rename = "Btu@47max", deserialize_with = "deserialize_f64_custom")]
    pub btu_47_max: Option<f64>,
}

impl MachineData {
    fn calculate_heating_capacity_at_temp(&self, target_temp: f64) -> f64 {
        let mut points = Vec::new();
        
        if let (Some(temp), Some(val)) = (self.lowest_temp, self.btu_lowest_max) {
             points.push((temp, val));
        }
        if let Some(val) = self.btu_5_max { points.push((5.0, val)); }
        if let Some(val) = self.btu_17_max { points.push((17.0, val)); }
        if let Some(val) = self.btu_47_max { points.push((47.0, val)); }

        if points.is_empty() { return 0.0; }
        if points.len() == 1 { return points[0].1; }

        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let (p1, p2) = if target_temp <= points[0].0 {
            (points[0], points[1])
        } else if target_temp >= points.last().unwrap().0 {
            let len = points.len();
            (points[len-2], points[len-1])
        } else {
            let mut found = (points[0], points[1]);
            for window in points.windows(2) {
                if target_temp >= window[0].0 && target_temp <= window[1].0 {
                    found = (window[0], window[1]);
                    break;
                }
            }
            found
        };

        let (x1, y1) = p1;
        let (x2, y2) = p2;
        
        if (x2 - x1).abs() < 1e-6 { return y1; }

        let slope = (y2 - y1) / (x2 - x1);
        y1 + (target_temp - x1) * slope
    }
}

#[derive(Debug, Default)]
struct CalculationTotals {
    total_btu_95_min: f64,
    total_btu_5_max: f64,
    total_btu_17_max: f64,
    total_btu_17_rated: f64,
    total_btu_design_max: f64,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None, name = "lc")]
pub struct Cli {
    #[arg(required = true, help = "机器列表 (e.g. KM18H5Ox1)")]
    pub machines: Vec<String>,

    /// Design temperature for heating calculation
    #[arg(short = 't', long, default_value_t = 17.0)]
    pub design_temp: f64,
}


fn parse_user_input(inputs: &[String]) -> Result<HashMap<String, u32>, String> {
    lazy_static! {
        // regex 1: [model]x[qty]
        static ref MODEL_QTY_RE: Regex = Regex::new(r"^(.+)x(\d+)$").unwrap();
        // regex 2: [code][qty]
        static ref CODE_QTY_RE: Regex = Regex::new(r"^([a-zA-Z0-9]+?)(\d+)$").unwrap();
    }
    
    let mut input_map = HashMap::new();

    for item in inputs {
        let (identifier, count_str) = if let Some(caps) = MODEL_QTY_RE.captures(item) {
            (caps[1].to_string(), caps[2].to_string())
        } else if CODE_QTY_RE.is_match(item) {
            let last_char_index = item.rfind(|c: char| !c.is_ascii_digit());
            if let Some(idx) = last_char_index {
                 if idx < item.len() - 1 {
                    let id = item[..idx+1].to_string();
                    let qty = item[idx+1..].to_string();
                    (id, qty)
                } else {
                    (item.clone(), "1".to_string())
                }
            } else {
                 return Err(format!("Format error: {}", item));
            }
        } else {
             (item.clone(), "1".to_string())
        };

        let count: u32 = count_str.parse().map_err(|_| "Qty must be integer")?;
        
        *input_map.entry(identifier).or_insert(0) += count;
    }
    Ok(input_map)
}

fn load_machine_data() -> Result<HashMap<String, MachineData>, Box<dyn std::error::Error>> {
    let mut reader = csv::Reader::from_reader(CSV_DATA.as_bytes());
    let mut data_map = HashMap::new();

    for result in reader.deserialize() {
        let record: MachineData = result.map_err(|e| format!("CSV Parse Error: {}", e))?;
        data_map.insert(record.model_number.clone(), record.clone());
        if let Some(code) = &record.machine_code {
            data_map.insert(code.clone(), record);
        }
    }
    Ok(data_map)
}

fn print_separator(widths: &[usize]) {
    let mut line = String::from("+");
    for w in widths {
        line.push_str(&"-".repeat(*w + 2));
        line.push('+');
    }
    println!("{}", line);
}

fn perform_calculation(
    user_input: &HashMap<String, u32>,
    machine_data: &HashMap<String, MachineData>,
    design_temp: f64,
) -> CalculationTotals {
    let mut totals = CalculationTotals::default();
    
    let col_widths = vec![13, 5, 12, 10, 10];
    let header_design_label = format!("Btu@{} max", design_temp);

    // 1. 预聚合 (Normalization & Aggregation)
    // 将输入统一解析为 Model Number，并累加数量
    let mut canonical_counts: HashMap<String, u32> = HashMap::new();
    let mut not_found_inputs: Vec<(&String, &u32)> = Vec::new();

    for (identifier, count) in user_input {
        if let Some(data) = machine_data.get(identifier) {
            *canonical_counts.entry(data.model_number.clone()).or_insert(0) += count;
        } else {
            not_found_inputs.push((identifier, count));
        }
    }

    print_separator(&col_widths);
    println!(
        "| {:^13} | {:^5} | {:^12} | {:^10} | {:^10} |", 
        "Model", "qty", "AHRI#", "Btu@95 min", header_design_label
    );
    print_separator(&col_widths);

    // 2. 遍历聚合后的型号进行计算和输出
    let mut sorted_models: Vec<_> = canonical_counts.into_iter().collect();
    sorted_models.sort_by(|a, b| a.0.cmp(&b.0)); // 按型号名称排序

    for (model_number, count) in sorted_models {
        if let Some(data) = machine_data.get(&model_number) {
            let qty = count as f64;
            
            let ahri = data.ahri.map(|v| v.to_string()).unwrap_or("-".to_string());
            let btu_95_min = data.btu_95_min.unwrap_or(0.0);
            let btu_design_max = data.calculate_heating_capacity_at_temp(design_temp);

            totals.total_btu_95_min += btu_95_min * qty;
            totals.total_btu_design_max += btu_design_max * qty;
            
            totals.total_btu_5_max += data.btu_5_max.unwrap_or(0.0) * qty;
            totals.total_btu_17_max += data.btu_17_max.unwrap_or(0.0) * qty;
            totals.total_btu_17_rated += data.btu_17_rated.unwrap_or(0.0) * qty;

            println!(
                "| {:^13} | {:^5} | {:^12} | {:^10.0} | {:^10.0} |", 
                data.model_number, 
                count, 
                ahri,
                btu_95_min * qty, 
                btu_design_max * qty
            );
        }
    }

    // 3. 输出未找到的项目
    for (identifier, count) in not_found_inputs {
        println!("| {:^13} | {:^5} | {:^12} | {:^10} | {:^10} |", identifier, count, "NOT FOUND", "-", "-");
    }

    print_separator(&col_widths);
    totals
}

fn print_summary_table(totals: &CalculationTotals, design_temp: f64) {
    let widths = vec![13, 8];
    
    println!();
    print_separator(&widths);
    
    let print_row = |label: &str, value: f64, is_temp: bool| {
        let val_str = if is_temp {
            format!("{:.0}", value)
        } else {
            format!("{:.0}", value)
        };
        println!("| {:<13} | {:>8} |", label, val_str);
    };

    print_row("Btu @95 min", totals.total_btu_95_min, false);
    print_row("Btu @5  max", totals.total_btu_5_max, false);
    print_row("Btu @17 max", totals.total_btu_17_max, false);
    print_row("Btu @17 rated", totals.total_btu_17_rated, false);
    print_row(&format!("Btu @{} max", design_temp), totals.total_btu_design_max, false);
    print_row("Design Temp", design_temp, true);

    print_separator(&widths);
}

fn print_recommendation(totals: &CalculationTotals) {
    let max_val = totals.total_btu_design_max;
    let mid_val = max_val / 1.1;
    let min_val = max_val / 1.2;

    println!("recommend range: {:.0} - {:.0} - {:.0}", min_val, mid_val, max_val);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    
    let machine_data_map = load_machine_data()?;
    let user_input_map = parse_user_input(&cli.machines).map_err(|e| e.to_string())?;
    
    let totals = perform_calculation(&user_input_map, &machine_data_map, cli.design_temp);

    print_summary_table(&totals, cli.design_temp);
    print_recommendation(&totals);

    Ok(())
}