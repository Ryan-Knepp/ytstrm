use std::sync::Arc;
use minijinja::Environment;
use anyhow::Result;

pub struct Templates {
    env: Environment<'static>,
}

impl Templates {
    pub fn new() -> Result<Self> {
        let mut env = Environment::new();
        env.set_loader(minijinja::path_loader("src/templates"));
        Ok(Self { env })
    }

    pub fn render(&self, template: &str, context: minijinja::value::Value) -> Result<String> {
        let tmpl = self.env.get_template(template)?;
        Ok(tmpl.render(context)?)
    }
}

pub type TemplateState = Arc<Templates>;