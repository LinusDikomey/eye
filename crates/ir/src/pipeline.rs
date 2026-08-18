use crate::{Environment, FunctionIr, ModuleId, Types, verify};

pub trait ModulePass: std::fmt::Debug {
    fn run(&self, env: &mut Environment, module: ModuleId);
}

pub trait FunctionPass<State = ()>: std::fmt::Debug {
    fn run(
        &self,
        env: &Environment,
        types: &Types,
        ir: FunctionIr,
        name: &str,
        state: &mut State,
    ) -> (FunctionIr, Option<Types>);
}

#[derive(Default)]
pub struct Pipeline<State = ()> {
    pub context: &'static str,
    steps: Vec<(Vec<Box<dyn FunctionPass<State>>>, Box<dyn ModulePass>)>,
    final_step: Vec<Box<dyn FunctionPass<State>>>,
}
impl<State> Pipeline<State> {
    pub fn new(context: &'static str) -> Self {
        Self {
            context,
            steps: Vec::new(),
            final_step: Vec::new(),
        }
    }

    pub fn add_function_pass(&mut self, pass: Box<dyn FunctionPass<State>>) {
        self.final_step.push(pass);
    }

    pub fn add_module_pass(&mut self, pass: Box<dyn ModulePass>) {
        let function_passes = std::mem::take(&mut self.final_step);
        self.steps.push((function_passes, pass));
    }
}

impl<State: Default> Pipeline<State> {
    pub fn process_module(&self, env: &mut Environment, module: ModuleId) {
        self.process_module_with_regs::<crate::mc::UnknownRegister>(env, module);
    }

    pub fn process_module_with_regs<R: crate::mc::Register>(
        &self,
        env: &mut Environment,
        module: ModuleId,
    ) {
        verify::module(env, module);
        for id in env[module].function_ids() {
            let function = &env[module][id];
            let Some(ir) = function.ir.get() else {
                continue;
            };
            // HACK: don't process _start, it's only emitted on linux and shouldn't be touched
            if &*function.name == "_start" {
                continue;
            }
            tracing::debug!(
                target: "passes",
                stage = self.context,
                function = env[module][id].name,
                "IR for {} before:\n{}",
                env[module][id].name,
                ir.display_with_phys_regs::<R>(env, &function.types),
            );
        }
        for (step, module_pass) in &self.steps {
            self.process_step::<R>(env, module, step);
            let _enter = tracing::span!(
                target: "optimize",
                tracing::Level::INFO,
                "module_pass",
                pass = ?module_pass
            )
            .entered();
            module_pass.run(env, module);
            for id in env[module].function_ids() {
                let Some(ir) = env[module][id].ir() else {
                    continue;
                };
                tracing::debug!(
                    target: "passes",
                    function = env[module][id].name,
                    "After {:?}:\n{}",
                    module_pass,
                    ir.display_with_phys_regs::<R>(env, &env[module][id].types),
                );
            }

            verify::module(env, module);
        }
        self.process_step::<R>(env, module, &self.final_step);
        verify::module(env, module);
    }

    fn process_step<R: crate::mc::Register>(
        &self,
        env: &mut Environment,
        module: ModuleId,
        step: &[Box<dyn FunctionPass<State>>],
    ) {
        for id in env[module].function_ids() {
            let Some(mut ir) = env.modules[module.idx()].functions[id.idx()].ir.take() else {
                continue;
            };
            let mut state = State::default();

            for pass in step {
                let _enter = tracing::span!(
                    target: "passes", tracing::Level::INFO, "Running function pass",
                    function = env[module][id].name,
                    pass = ?pass,
                )
                .entered();
                let types;
                let func = &env[module][id];
                (ir, types) = pass.run(env, &func.types, ir, &func.name, &mut state);
                if let Some(types) = types {
                    env.modules[module.idx()].functions[id.idx()].types = types;
                }
                tracing::debug!(
                    target: "passes",
                    function = env[module][id].name,
                    pass = ?pass,
                    "After {pass:?}:\n{}",
                    ir.display_with_phys_regs::<R>(env, &env[module][id].types),
                );
                verify::function_body(env, &ir, env[module][id].types(), &env[module][id].name);
            }
            env.modules[module.idx()][id]
                .ir
                .set(ir)
                .unwrap_or_else(|_| panic!("function was set twice"));
        }
    }

    pub fn process_function(
        &self,
        env: &Environment,
        ir: FunctionIr,
        types: &mut Types,
        name: &str,
    ) -> FunctionIr {
        self.process_function_with_regs::<crate::mc::UnknownRegister>(env, ir, types, name)
    }

    pub fn process_function_with_regs<R: crate::mc::Register>(
        &self,
        env: &Environment,
        mut ir: FunctionIr,
        types: &mut Types,
        name: &str,
    ) -> FunctionIr {
        assert!(
            self.steps.is_empty(),
            "Pipeline contains module passes, can't process single function"
        );
        tracing::debug!(target: "passes", function = name, "IR before {}:\n{}", self.context, ir.display_with_phys_regs::<R>(env, types));

        let mut state = State::default();
        for pass in &self.final_step {
            let new_types;
            (ir, new_types) = pass.run(env, types, ir, name, &mut state);
            if let Some(new_types) = new_types {
                *types = new_types;
            }
            tracing::debug!(target: "passes", pass=?pass, function = name, "After {pass:?}:\n{}", ir.display_with_phys_regs::<R>(env, types));
        }
        ir
    }
}
