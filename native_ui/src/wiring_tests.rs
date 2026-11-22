use std::collections::BTreeSet;
use syn::{parse::Parser, visit::Visit, Expr, Lit, Pat};

#[derive(Default)]
struct Actions {
    exact: BTreeSet<String>,
    prefixes: BTreeSet<String>,
}

fn action_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Path(path) => path.path.is_ident("action") || path.path.is_ident("command"),
        Expr::MethodCall(call) if call.method == "as_str" => action_expr(&call.receiver),
        _ => false,
    }
}

fn literal(expr: &Expr) -> Option<String> {
    if let Expr::Lit(expr) = expr {
        if let Lit::Str(value) = &expr.lit {
            return Some(value.value());
        }
    }
    None
}

impl Actions {
    fn pattern(&mut self, pattern: &Pat) {
        match pattern {
            Pat::Lit(expr) => {
                if let Lit::Str(value) = &expr.lit {
                    self.exact.insert(value.value());
                }
            }
            Pat::Or(pattern) => {
                for case in &pattern.cases {
                    self.pattern(case);
                }
            }
            _ => {}
        }
    }

    fn handles(&self, action: &str) -> bool {
        self.exact.contains(action) || self.prefixes.iter().any(|p| action.starts_with(p))
    }

    fn extend(&mut self, other: Self) {
        self.exact.extend(other.exact);
        self.prefixes.extend(other.prefixes);
    }
}

impl<'ast> Visit<'ast> for Actions {
    fn visit_expr_match(&mut self, expr: &'ast syn::ExprMatch) {
        if action_expr(&expr.expr) {
            for arm in &expr.arms {
                self.pattern(&arm.pat);
                if let Some((_, guard)) = &arm.guard {
                    struct Prefixes<'a>(&'a mut BTreeSet<String>);
                    impl<'ast> Visit<'ast> for Prefixes<'_> {
                        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
                            if call.method == "starts_with" {
                                if let Some(value) = call.args.first().and_then(literal) {
                                    self.0.insert(value);
                                }
                            }
                            syn::visit::visit_expr_method_call(self, call);
                        }
                    }
                    Prefixes(&mut self.prefixes).visit_expr(guard);
                }
            }
        }
        syn::visit::visit_expr_match(self, expr);
    }

    fn visit_expr_binary(&mut self, expr: &'ast syn::ExprBinary) {
        if matches!(expr.op, syn::BinOp::Eq(_)) && action_expr(&expr.left) {
            if let Some(value) = literal(&expr.right) {
                self.exact.insert(value);
            }
        }
        syn::visit::visit_expr_binary(self, expr);
    }

    fn visit_expr_macro(&mut self, expr: &'ast syn::ExprMacro) {
        if expr.mac.path.is_ident("matches") {
            let parser = |input: syn::parse::ParseStream<'_>| {
                let value: Expr = input.parse()?;
                input.parse::<syn::Token![,]>()?;
                let pattern = Pat::parse_multi_with_leading_vert(input)?;
                Ok((value, pattern))
            };
            if let Ok((value, pattern)) = parser.parse2(expr.mac.tokens.clone()) {
                if action_expr(&value) {
                    self.pattern(&pattern);
                }
            }
        }
    }
}

fn dispatcher(source: &str, names: &[&str]) -> Actions {
    let file = syn::parse_file(source).unwrap();
    let mut actions = Actions::default();
    let mut found = BTreeSet::new();
    for item in &file.items {
        match item {
            syn::Item::Fn(function) if names.iter().any(|name| function.sig.ident == name) => {
                found.insert(function.sig.ident.to_string());
                actions.visit_block(&function.block);
            }
            syn::Item::Impl(item) => {
                for item in &item.items {
                    if let syn::ImplItem::Fn(function) = item {
                        if names.iter().any(|name| function.sig.ident == name) {
                            found.insert(function.sig.ident.to_string());
                            actions.visit_block(&function.block);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(found.len(), names.len(), "dispatcher renamed: {names:?}");
    actions
}

fn callback_dispatcher(source: &str, receiver: &str) -> Actions {
    struct Callback<'a> {
        receiver: &'a str,
        actions: Actions,
        found: usize,
    }
    impl<'ast> Visit<'ast> for Callback<'_> {
        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            if call.method == "on_command"
                && matches!(&*call.receiver, Expr::Path(path) if path.path.is_ident(self.receiver))
            {
                self.found += 1;
                for argument in &call.args {
                    self.actions.visit_expr(argument);
                }
            }
            syn::visit::visit_expr_method_call(self, call);
        }
    }
    let mut visitor = Callback {
        receiver,
        actions: Actions::default(),
        found: 0,
    };
    visitor.visit_file(&syn::parse_file(source).unwrap());
    assert_eq!(
        visitor.found, 1,
        "missing or ambiguous {receiver}.on_command"
    );
    visitor.actions
}

fn method_callback_dispatcher(source: &str, method: &str, callback: &str) -> Actions {
    struct Callback<'a> {
        method: &'a str,
        callback: &'a str,
        actions: Actions,
        found: usize,
    }
    impl<'ast> Visit<'ast> for Callback<'_> {
        fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
            if function.sig.ident == self.method {
                self.visit_block(&function.block);
            }
        }
        fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
            if call.method == self.callback {
                self.found += 1;
                for argument in &call.args {
                    self.actions.visit_expr(argument);
                }
            }
            syn::visit::visit_expr_method_call(self, call);
        }
    }
    let mut visitor = Callback {
        method,
        callback,
        actions: Actions::default(),
        found: 0,
    };
    visitor.visit_file(&syn::parse_file(source).unwrap());
    assert_eq!(visitor.found, 1, "missing or ambiguous {method}/{callback}");
    visitor.actions
}

fn model_actions(source: &str, name: &str, tuple_index: Option<usize>) -> BTreeSet<String> {
    struct Model<'a> {
        name: &'a str,
        tuple_index: Option<usize>,
        values: BTreeSet<String>,
    }
    impl<'ast> Visit<'ast> for Model<'_> {
        fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
            if function.sig.ident == self.name {
                self.visit_block(&function.block);
            }
        }
        fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
            if function.sig.ident == self.name {
                self.visit_block(&function.block);
            }
        }
        fn visit_expr_array(&mut self, array: &'ast syn::ExprArray) {
            for element in &array.elems {
                let value = match (self.tuple_index, element) {
                    (Some(index), Expr::Tuple(tuple)) => {
                        tuple.elems.iter().nth(index).and_then(literal)
                    }
                    (None, value) => literal(value),
                    _ => None,
                };
                if let Some(value) = value {
                    self.values.insert(value);
                }
            }
        }
    }
    let mut visitor = Model {
        name,
        tuple_index,
        values: BTreeSet::new(),
    };
    visitor.visit_file(&syn::parse_file(source).unwrap());
    assert!(
        !visitor.values.is_empty(),
        "no model actions found in {name}"
    );
    visitor.values
}

fn component<'a>(source: &'a str, name: &str) -> &'a str {
    source
        .split("export component ")
        .find(|part| part.starts_with(name))
        .unwrap()
}

fn tokens(source: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c == '/' && chars.peek().is_some_and(|(_, c)| *c == '/') {
            for (_, c) in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
        } else if c == '"' {
            let mut escaped = false;
            for (end, c) in chars.by_ref() {
                if c == '"' && !escaped {
                    tokens.push(source[start..=end].to_owned());
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
        } else if c.is_alphanumeric() || c == '_' {
            let mut end = start + c.len_utf8();
            while chars
                .peek()
                .is_some_and(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '-')
            {
                let (index, c) = chars.next().unwrap();
                end = index + c.len_utf8();
            }
            tokens.push(source[start..end].to_owned());
        } else if !c.is_whitespace() {
            tokens.push(c.to_string());
        }
    }
    tokens
}

fn strings(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .filter_map(|token| syn::parse_str::<syn::LitStr>(token).ok().map(|s| s.value()))
        .collect()
}

fn emitted(source: &str) -> BTreeSet<String> {
    let tokens = tokens(source);
    let mut result = BTreeSet::new();
    for index in 0..tokens.len().saturating_sub(1) {
        if matches!(tokens[index].as_str(), "action" | "command") && tokens[index + 1] == "(" {
            let mut depth = 1;
            let start = index + 2;
            let mut end = start;
            while depth != 0 {
                match tokens[end].as_str() {
                    "(" => depth += 1,
                    ")" => depth -= 1,
                    _ => {}
                }
                end += 1;
            }
            let arguments = &tokens[start..end - 1];
            let values = if arguments.get(1).is_some_and(|s| s == "+") {
                strings(&arguments[..1])
            } else {
                strings(arguments)
            };
            result.extend(values.into_iter().filter(|value| !value.is_empty()));
        }
        if tokens[index] == "for" && tokens.get(index + 3).is_some_and(|s| s == "[") {
            let start = index + 4;
            let end = start + tokens[start..].iter().position(|s| s == "]").unwrap();
            let variable = &tokens[index + 1];
            let body = &tokens[end..];
            if body
                .windows(3)
                .any(|w| w[0] == "action" && w[1] == "(" && &w[2] == variable)
            {
                result.extend(strings(&tokens[start..end]));
            }
        }
    }
    result
}

fn check(label: &str, slint: &str, actions: &Actions) {
    let emitted = emitted(slint);
    assert!(!emitted.is_empty(), "no actions extracted from {label}");
    let missing: Vec<_> = emitted
        .iter()
        .filter(|action| !actions.handles(action))
        .collect();
    assert!(
        missing.is_empty(),
        "{label}: unhandled UI actions: {missing:?}"
    );
}

#[test]
fn slint_actions_reach_rust_dispatchers() {
    let workbench = include_str!("workbench.rs");
    let main = include_str!("hot_main.rs");
    let hot = include_str!("../ui/hot.slint");
    let settings = method_callback_dispatcher(workbench, "open_settings", "on_action");
    assert!(!settings.handles("credentials"));
    let mut rules = dispatcher(include_str!("settings_editor.rs"), &["rule_action"]);
    rules.extend(method_callback_dispatcher(
        workbench,
        "open_settings",
        "on_site_rule_action",
    ));
    let mut queues = dispatcher(include_str!("queue_editor.rs"), &["edit"]);
    queues.extend(dispatcher(workbench, &["open_queues"]));
    let mut media = dispatcher(include_str!("media_controller.rs"), &["action"]);
    media.extend(dispatcher(
        include_str!("../../native_shell/src/download_worker.rs"),
        &["player_control"],
    ));
    for (label, source, actions) in [
        (
            "workbench",
            include_str!("../ui/workbench.slint"),
            dispatcher(workbench, &["action"]),
        ),
        ("settings", include_str!("../ui/settings.slint"), settings),
        ("site rules", include_str!("../ui/site_rules.slint"), rules),
        (
            "editor",
            include_str!("../ui/task_editor.slint"),
            dispatcher(workbench, &["editor_action"]),
        ),
        (
            "details",
            include_str!("../ui/details.slint"),
            dispatcher(workbench, &["open_details"]),
        ),
        ("queues", include_str!("../ui/queues.slint"), queues),
        (
            "resources",
            include_str!("../ui/resource_tools.slint"),
            dispatcher(include_str!("resource_tools.rs"), &["action"]),
        ),
        (
            "system",
            include_str!("../ui/system.slint"),
            dispatcher(include_str!("system_tools.rs"), &["action"]),
        ),
        (
            "services",
            include_str!("../ui/service_settings.slint"),
            dispatcher(include_str!("service_settings.rs"), &["action"]),
        ),
        (
            "cast",
            include_str!("../ui/cast_tools.slint"),
            dispatcher(include_str!("cast_tools.rs"), &["action"]),
        ),
        ("media", include_str!("../ui/media.slint"), media),
    ] {
        check(label, source, &actions);
    }
    for (name, receiver) in [
        ("ConfirmWindow", "confirm"),
        ("ProgressWindow", "progress"),
        ("CompleteWindow", "complete"),
    ] {
        check(
            name,
            component(hot, name),
            &callback_dispatcher(main, receiver),
        );
    }
    let actions = dispatcher(workbench, &["action"]);
    for (name, index) in [("render_toolbar", 0), ("render_menu", 1)] {
        for action in model_actions(workbench, name, Some(index)) {
            assert!(
                actions.handles(&action),
                "{name}: unhandled model action {action}"
            );
        }
    }
    let controls = model_actions(
        include_str!("../../native_shell/src/cast.rs"),
        "supported_actions",
        None,
    );
    for action in emitted(include_str!("../ui/cast_tools.slint")) {
        if let Some(control) = action.strip_prefix("control:") {
            assert!(
                controls.contains(control),
                "Core cannot handle cast action {action}"
            );
        }
    }
}

#[test]
fn every_slint_action_surface_is_audited() {
    let audited = [
        "workbench",
        "settings",
        "site_rules",
        "task_editor",
        "details",
        "queues",
        "resource_tools",
        "system",
        "service_settings",
        "cast_tools",
        "media",
        "hot",
    ];
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "slint")
            && !emitted(&std::fs::read_to_string(&path).unwrap()).is_empty()
        {
            assert!(
                audited.contains(&path.file_stem().unwrap().to_str().unwrap()),
                "add {} to the dispatcher audit",
                path.display()
            );
        }
    }
}

#[test]
fn scanner_rejects_unknown_actions_and_ignores_comments_and_forwarders() {
    assert!(
        emitted("// root.action(\"typo\");\naction(value) => { root.action(value); }").is_empty()
    );
    let actions = dispatcher("fn action(action: &str) { match action { \"save\" | \"close\" => (), value if value.starts_with(\"task:\") => (), _ => () } }", &["action"]);
    assert!(actions.handles("save"));
    assert!(actions.handles("task:pause"));
    assert!(!actions.handles("typo"));
    assert!(!actions.handles(""));
    assert_eq!(
        emitted("root.action(ok ? \"save\" : \"close\"); root.action(\"task:\" + item);"),
        BTreeSet::from(["save".into(), "close".into(), "task:".into()])
    );
}
