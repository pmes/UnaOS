use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use boa_engine::{
    Context, JsResult, JsValue, JsString, js_string,
    class::{Class, ClassBuilder},
    native_function::NativeFunction,
    Trace, Finalize,
};

#[derive(Debug, Trace, Finalize)]
pub struct LocalStorage {
    #[unsafe_ignore_trace]
    data: HashMap<String, String>,
    #[unsafe_ignore_trace]
    path: PathBuf,
}

impl boa_engine::object::JsData for LocalStorage {}

impl LocalStorage {
    pub fn new(path: PathBuf) -> Self {
        let data = if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                serde_json::from_str(&content).unwrap_or_default()
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };
        Self { data, path }
    }

    fn save(&self) {
        if let Ok(content) = serde_json::to_string(&self.data) {
            let _ = fs::write(&self.path, content);
        }
    }
}

impl Class for LocalStorage {
    const NAME: &'static str = "Storage";
    const LENGTH: usize = 0;

    fn data_constructor(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<Self> {
        let origin = if let Some(arg) = args.get(0) {
            arg.to_string(context)?.to_std_string_escaped()
        } else {
            "default_origin".to_string()
        };
        let sanitized = origin.replace(|c: char| !c.is_alphanumeric(), "_");
        let dir = directories::ProjectDirs::from("", "UnaOS", "Aether")
            .map(|p| p.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/tmp/aether_storage"));
        let _ = std::fs::create_dir_all(&dir);
        Ok(LocalStorage::new(dir.join(format!("{}.json", sanitized))))
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(js_string!("getItem"), 1, NativeFunction::from_fn_ptr(Self::get_item))
             .method(js_string!("setItem"), 2, NativeFunction::from_fn_ptr(Self::set_item))
             .method(js_string!("removeItem"), 1, NativeFunction::from_fn_ptr(Self::remove_item))
             .method(js_string!("clear"), 0, NativeFunction::from_fn_ptr(Self::clear));
        Ok(())
    }
}

impl LocalStorage {
    fn get_item(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        if let Some(object) = this.as_object() {
            if let Some(storage) = object.downcast_ref::<LocalStorage>() {
                if let Some(key_val) = args.get(0) {
                    let key = key_val.to_string(context)?.to_std_string_escaped();
                    if let Some(value) = storage.data.get(&key) {
                        return Ok(JsValue::from(JsString::from(value.as_str())));
                    }
                }
            }
        }
        Ok(JsValue::null())
    }

    fn set_item(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        if let Some(object) = this.as_object() {
            if let Some(mut storage) = object.downcast_mut::<LocalStorage>() {
                if let (Some(key_val), Some(val_val)) = (args.get(0), args.get(1)) {
                    let key = key_val.to_string(context)?.to_std_string_escaped();
                    let value = val_val.to_string(context)?.to_std_string_escaped();
                    storage.data.insert(key, value);
                    storage.save();
                }
            }
        }
        Ok(JsValue::undefined())
    }

    fn remove_item(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
        if let Some(object) = this.as_object() {
            if let Some(mut storage) = object.downcast_mut::<LocalStorage>() {
                if let Some(key_val) = args.get(0) {
                    let key = key_val.to_string(context)?.to_std_string_escaped();
                    storage.data.remove(&key);
                    storage.save();
                }
            }
        }
        Ok(JsValue::undefined())
    }

    fn clear(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
        if let Some(object) = this.as_object() {
            if let Some(mut storage) = object.downcast_mut::<LocalStorage>() {
                storage.data.clear();
                storage.save();
            }
        }
        Ok(JsValue::undefined())
    }
}
