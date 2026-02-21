use crate::inode::AttributeValue;

#[derive(Debug, Clone, PartialEq)]
pub enum QueryOp {
    Eq,
    Neq,
    Gt,
    Lt,
    // Special ops
    SimilarityGt(f32), // similarity(key, vec) > threshold
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub key: String,
    pub op: QueryOp,
    pub value: AttributeValue, // For Similarity, this holds the target vector
}

impl Query {
    pub fn parse(input: &str) -> Result<Self, String> {
        let input = input.trim();

        // Check for function call syntax first: similarity(key, [vec]) > threshold
        if input.starts_with("similarity(") {
            return parse_similarity(input);
        }

        // Simple binary ops: key op value
        // Split by operators
        let ops = ["==", "!=", ">", "<"];
        for op in ops {
            if let Some(idx) = input.find(op) {
                let key_part = input[..idx].trim();
                let val_part = input[idx+op.len()..].trim();

                let key = key_part.to_string();
                let value = parse_value(val_part)?;

                let query_op = match op {
                    "==" => QueryOp::Eq,
                    "!=" => QueryOp::Neq,
                    ">" => QueryOp::Gt,
                    "<" => QueryOp::Lt,
                    _ => return Err("Unknown operator".to_string()),
                };

                return Ok(Query {
                    key,
                    op: query_op,
                    value,
                });
            }
        }

        Err("Invalid query syntax".to_string())
    }
}

pub fn parse_value(input: &str) -> Result<AttributeValue, String> {
    let input = input.trim();
    if input.starts_with('"') && input.ends_with('"') {
        // String
        let inner = &input[1..input.len()-1];
        Ok(AttributeValue::String(inner.to_string()))
    } else if input.starts_with('[') && input.ends_with(']') {
        // Vector
        let inner = &input[1..input.len()-1];
        let parts: Vec<&str> = inner.split(',').collect();
        let mut vec = Vec::new();
        for p in parts {
            let f = p.trim().parse::<f32>().map_err(|_| "Invalid number in vector")?;
            vec.push(f);
        }
        Ok(AttributeValue::Vector(vec))
    } else if let Ok(i) = input.parse::<i64>() {
        Ok(AttributeValue::Int(i))
    } else if let Ok(f) = input.parse::<f32>() {
        Ok(AttributeValue::Float(f as f64))
    } else {
        // Treat unquoted as string? Or error?
        // Let's treat as string for convenience if it looks like a word
        Ok(AttributeValue::String(input.to_string()))
    }
}

fn parse_similarity(input: &str) -> Result<Query, String> {
    // Format: similarity(key, [vec]) > threshold
    // 1. Split by ">"
    let parts: Vec<&str> = input.split('>').collect();
    if parts.len() != 2 {
        return Err("Similarity query must use '>' operator".to_string());
    }

    let lhs = parts[0].trim(); // similarity(key, [vec])
    let rhs = parts[1].trim(); // threshold

    let threshold = rhs.parse::<f32>().map_err(|_| "Invalid threshold")?;

    // Parse LHS
    if !lhs.ends_with(')') {
        return Err("Malformed function call".to_string());
    }
    let args_str = &lhs["similarity(".len()..lhs.len()-1];

    // args: key, [vec]
    // Find comma separating key and vec
    // Be careful if key is quoted string containing comma? We assume simple keys.
    // Or just look for first comma.
    let comma_idx = args_str.find(',').ok_or("Missing comma in similarity args")?;
    let key = args_str[..comma_idx].trim().to_string();
    let vec_str = args_str[comma_idx+1..].trim();

    let value = parse_value(vec_str)?;
    if !matches!(value, AttributeValue::Vector(_)) {
        return Err("Second argument to similarity must be a vector".to_string());
    }

    Ok(Query {
        key,
        op: QueryOp::SimilarityGt(threshold),
        value,
    })
}
