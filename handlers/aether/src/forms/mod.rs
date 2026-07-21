#[derive(Debug, Clone, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenDocument {
    pub url: String,
    pub method: HttpMethod,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FormInput {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct Form {
    pub action: String,
    pub method: HttpMethod,
    pub inputs: Vec<FormInput>,
}

impl Form {
    pub fn new(action: String, method: HttpMethod) -> Self {
        Self {
            action,
            method,
            inputs: Vec::new(),
        }
    }

    pub fn add_input(&mut self, name: String, value: String) {
        self.inputs.push(FormInput { name, value });
    }

    pub fn submit(&self) -> OpenDocument {
        let mut query_string = String::new();
        for (i, input) in self.inputs.iter().enumerate() {
            if i > 0 {
                query_string.push('&');
            }
            query_string.push_str(&format!("{}={}", input.name, input.value));
        }

        match self.method {
            HttpMethod::Get => {
                let url = if self.action.contains('?') {
                    format!("{}&{}", self.action, query_string)
                } else {
                    if query_string.is_empty() {
                        self.action.clone()
                    } else {
                        format!("{}?{}", self.action, query_string)
                    }
                };

                OpenDocument {
                    url,
                    method: HttpMethod::Get,
                    body: None,
                }
            }
            HttpMethod::Post => OpenDocument {
                url: self.action.clone(),
                method: HttpMethod::Post,
                body: if query_string.is_empty() {
                    None
                } else {
                    Some(query_string)
                },
            },
        }
    }
}
