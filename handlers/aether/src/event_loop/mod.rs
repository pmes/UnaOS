use boa_engine::{
    context::ContextBuilder,
    job::{SimpleJobExecutor, NativeJob},
    native_function::NativeFunction,
    Context, JsData, JsResult, JsValue, Trace, Finalize, JsString
};
use std::rc::Rc;

#[derive(Trace, Finalize, JsData)]
pub struct TimerData {
    pub id: u32,
}

pub fn set_timeout(
    _this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    if let Some(callback) = args.get(0).and_then(|v| v.as_callable()) {
        let callback = callback.clone();
        context.enqueue_job(boa_engine::job::Job::from(boa_engine::job::GenericJob::new(move |context| {
            let _ = callback.call(&JsValue::undefined(), &[], context);
            Ok(JsValue::undefined())
        }, context.realm().clone())));
    }
    Ok(JsValue::new(1))
}

pub fn create_context() -> Context {
    let executor = SimpleJobExecutor::new();
    let mut context = ContextBuilder::new()
        .job_executor(Rc::new(executor))
        .build()
        .unwrap();

    let name: JsString = "setTimeout".into();
    context.register_global_callable(name, 1, NativeFunction::from_fn_ptr(set_timeout)).unwrap();

    context
}
