use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use proc_macro2::{LineColumn, Span};
use syn::{
    Attribute, ImplItem, Item, ItemMod, Lit, Meta, Token, UseTree, ext::IdentExt, parse::Parser,
    punctuated::Punctuated, spanned::Spanned,
};

use crate::{
    analysis::GraphBuilder,
    diagnostics::{AnalysisStatus, DiagnosticPhase, Severity},
    discovery::{diagnostic, module_edge, source_file_node},
    model::{
        CrateId, DefinitionId, DefinitionKind, DefinitionNode, DefinitionVisibility, EdgeKind,
        Evidence, EvidenceKind, ModuleId, ModuleNode, Node, NodeId, Provenance,
        RelationshipCondition, RepoPath, ResolutionStatus, SourceSpan,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModuleDiscovery {
    pub status: AnalysisStatus,
    pub dependency_status: AnalysisStatus,
}

struct Context {
    module_id: ModuleId,
    logical_path: String,
    source: PathBuf,
    resolution_path: PathBuf,
    source_rel: RepoPath,
    file_rel: RepoPath,
    module_dir: PathBuf,
    inline_depth: usize,
    inherited_condition: Option<RelationshipCondition>,
    module_dir_ambiguous: bool,
}

struct Walker<'a> {
    builder: &'a mut GraphBuilder,
    root: &'a Path,
    crate_id: CrateId,
    status: AnalysisStatus,
    dependency_status: AnalysisStatus,
    visited: BTreeSet<(ModuleId, PathBuf, PathBuf)>,
    stack: Vec<PathBuf>,
    modules: BTreeMap<String, ModuleId>,
    definitions: DefinitionsByPath,
    module_conditions: BTreeMap<String, BTreeSet<Option<RelationshipCondition>>>,
    module_visibility: BTreeMap<String, BTreeSet<DefinitionVisibility>>,
    pending_imports: Vec<PendingImport>,
    edition: String,
    external_crates: BTreeSet<String>,
}

type DefinitionsByPath =
    BTreeMap<(String, String), Vec<(NodeId, Option<RelationshipCondition>, DefinitionVisibility)>>;

#[derive(Clone)]
struct PendingImport {
    module: ModuleId,
    logical_module: String,
    path: Vec<String>,
    absolute: bool,
    spelling: String,
    alias: Option<String>,
    reexport: bool,
    condition: Option<RelationshipCondition>,
    evidence: Evidence,
    edition: String,
}

#[derive(Default)]
struct AliasResolution {
    resolved: BTreeMap<(String, String), String>,
    declared: BTreeSet<(String, String)>,
    ambiguous: BTreeSet<(String, String)>,
}

pub(crate) fn discover_crate_modules(
    builder: &mut GraphBuilder,
    repository_root: &Path,
    root_path: &RepoPath,
    crate_id: &CrateId,
    edition: &str,
    external_crates: BTreeSet<String>,
) -> ModuleDiscovery {
    let full_root = repository_root.join(root_path.as_str());
    let canonical = match canonical_source(repository_root, &full_root) {
        Ok(path) => path,
        Err((code, message)) => {
            builder.add_diagnostic(diagnostic(
                code,
                Severity::Error,
                DiagnosticPhase::Discovery,
                message,
                Some(root_path.clone()),
                None,
            ));
            return ModuleDiscovery {
                status: AnalysisStatus::Partial,
                dependency_status: AnalysisStatus::Partial,
            };
        }
    };
    let file_rel = match relative_to(repository_root, &canonical) {
        Ok(path) => path,
        Err(message) => {
            builder.add_diagnostic(diagnostic(
                "SOURCE_OUTSIDE_REPOSITORY",
                Severity::Error,
                DiagnosticPhase::Discovery,
                message,
                Some(root_path.clone()),
                None,
            ));
            return ModuleDiscovery {
                status: AnalysisStatus::Partial,
                dependency_status: AnalysisStatus::Partial,
            };
        }
    };
    let source_rel = root_path.clone();
    let root_module = ModuleId::from(format!("{}::module:crate", crate_id.0));
    let root_node = Node::Module(ModuleNode {
        id: root_module.clone(),
        crate_id: crate_id.clone(),
        logical_path: "crate".into(),
    });
    if let Err(error) = builder.add_node(root_node) {
        builder.add_diagnostic(diagnostic(
            "GRAPH_CONSTRUCTION_FAILED",
            Severity::Error,
            DiagnosticPhase::GraphConstruction,
            error.to_string(),
            None,
            None,
        ));
        return ModuleDiscovery {
            status: AnalysisStatus::Partial,
            dependency_status: AnalysisStatus::Partial,
        };
    }
    let crate_node = NodeId::Crate(crate_id.clone());
    let module_node = NodeId::Module(root_module.clone());
    let metadata_evidence = Evidence {
        kind: EvidenceKind::PackageMetadata,
        provenance: Provenance::CargoMetadata,
        path: Some(source_rel.clone()),
        span: None,
    };
    let source_mapping_evidence = Evidence {
        kind: EvidenceKind::SourceMapping,
        provenance: Provenance::CargoMetadata,
        path: Some(source_rel.clone()),
        span: None,
    };
    let contains = module_edge(
        EdgeKind::Contains,
        crate_node,
        Some(module_node.clone()),
        None,
        metadata_evidence,
        None,
    );
    if let Err(error) = builder.add_edge(contains) {
        builder.add_diagnostic(diagnostic(
            "GRAPH_CONSTRUCTION_FAILED",
            Severity::Error,
            DiagnosticPhase::GraphConstruction,
            error.to_string(),
            None,
            None,
        ));
    }
    let root_mapping = module_edge(
        EdgeKind::LoadsSource,
        module_node,
        Some(NodeId::SourceFile(crate::model::SourceFileId::from(
            format!("file:{}", file_rel.as_str()),
        ))),
        None,
        source_mapping_evidence,
        None,
    );
    // The file node is installed by `walk_file` so a missing root cannot be fabricated.
    let mut walker = Walker {
        builder,
        root: repository_root,
        crate_id: crate_id.clone(),
        status: AnalysisStatus::Complete,
        dependency_status: AnalysisStatus::Complete,
        visited: BTreeSet::new(),
        stack: Vec::new(),
        modules: BTreeMap::from([("crate".into(), root_module.clone())]),
        definitions: BTreeMap::new(),
        module_conditions: BTreeMap::from([("crate".into(), BTreeSet::from([None]))]),
        module_visibility: BTreeMap::from([(
            "crate".into(),
            BTreeSet::from([DefinitionVisibility::Public]),
        )]),
        pending_imports: Vec::new(),
        edition: edition.to_owned(),
        external_crates,
    };
    walker.walk_file(
        Context {
            module_id: root_module,
            logical_path: "crate".into(),
            source: canonical.clone(),
            resolution_path: full_root.clone(),
            source_rel,
            file_rel,
            module_dir: full_root.parent().unwrap_or(repository_root).to_path_buf(),
            inline_depth: 0,
            inherited_condition: None,
            module_dir_ambiguous: false,
        },
        Some(root_mapping),
    );
    walker.resolve_imports();
    ModuleDiscovery {
        status: walker.status,
        dependency_status: walker.dependency_status,
    }
}

impl Walker<'_> {
    fn walk_file(&mut self, context: Context, initial_mapping: Option<crate::model::Edge>) {
        let file_node = source_file_node(context.file_rel.clone());
        let file_id = file_node.id();
        if let Err(error) = self.builder.add_node(file_node) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                Some(context.source_rel.clone()),
                None,
            );
            self.status = AnalysisStatus::Partial;
            return;
        }
        if let Some(mut mapping) = initial_mapping {
            mapping.target = Some(file_id);
            if let Err(error) = self.builder.add_edge(mapping) {
                self.report(
                    "GRAPH_CONSTRUCTION_FAILED",
                    Severity::Error,
                    error.to_string(),
                    Some(context.source_rel.clone()),
                    None,
                );
                self.status = AnalysisStatus::Partial;
                return;
            }
        }
        if self.stack.contains(&context.source) {
            self.report(
                "MODULE_SOURCE_CYCLE",
                Severity::Warning,
                "module source recursively includes a file already on the active module path",
                Some(context.source_rel.clone()),
                None,
            );
            self.status = AnalysisStatus::Partial;
            return;
        }
        if !self.visited.insert((
            context.module_id.clone(),
            context.source.clone(),
            context.module_dir.clone(),
        )) {
            return;
        }
        let bytes = match fs::read(&context.source) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.report(
                    "SOURCE_INACCESSIBLE",
                    Severity::Error,
                    format!("cannot read module source ({})", error.kind()),
                    Some(context.source_rel.clone()),
                    None,
                );
                self.status = AnalysisStatus::Partial;
                return;
            }
        };
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => {
                self.report(
                    "SOURCE_NOT_UTF8",
                    Severity::Error,
                    "Rust source file is not valid UTF-8",
                    Some(context.source_rel.clone()),
                    None,
                );
                self.status = AnalysisStatus::Partial;
                return;
            }
        };
        let parsed = match syn::parse_file(&content) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.report(
                    "RUST_PARSE_FAILED",
                    Severity::Error,
                    format!("Rust source could not be parsed: {}", error),
                    Some(context.source_rel.clone()),
                    Some(span_to_source(error.span(), &content)),
                );
                self.status = AnalysisStatus::Partial;
                return;
            }
        };
        let crate_condition = match condition_from_attributes(&parsed.attrs) {
            Ok(condition) => condition,
            Err(()) => {
                self.report(
                    "CFG_EXPRESSION_UNSUPPORTED",
                    Severity::Warning,
                    "crate-level cfg expression could not be normalized",
                    Some(context.source_rel.clone()),
                    None,
                );
                self.status = AnalysisStatus::Partial;
                Some(RelationshipCondition::Unknown)
            }
        };
        let unsupported_crate_attributes = unsupported_crate_attributes(&parsed.attrs);
        for attribute in &unsupported_crate_attributes {
            self.report(
                "CRATE_ATTRIBUTE_UNSUPPORTED",
                Severity::Warning,
                format!("crate attribute '{attribute}' is not interpreted and may affect module structure"),
                Some(context.source_rel.clone()),
                None,
            );
            self.status = AnalysisStatus::Partial;
        }
        let mut context = context;
        context.inherited_condition =
            combine_conditions(context.inherited_condition.clone(), crate_condition);
        if !unsupported_crate_attributes.is_empty() {
            context.inherited_condition = combine_conditions(
                context.inherited_condition,
                Some(RelationshipCondition::Unknown),
            );
        }
        self.stack.push(context.source.clone());
        self.process_items(&context, &parsed.items, &content);
        self.stack.pop();
    }

    fn process_items(&mut self, context: &Context, items: &[Item], content: &str) {
        for item in items {
            match item {
                Item::Mod(module) => self.process_module(context, module, content),
                Item::Use(item) => self.collect_import(context, item, content),
                Item::Macro(mac) if mac.ident.is_none() => {
                    self.report("MODULE_MACRO_UNEXPANDED", Severity::Warning, "item macro was not expanded; it may define modules that are absent from this graph", Some(context.source_rel.clone()), Some(span_to_source(mac.span(), content)));
                    self.status = AnalysisStatus::Partial;
                }
                Item::Macro(_) => self.collect_definition(context, item, content),
                _ => self.collect_definition(context, item, content),
            }
        }
    }

    fn collect_definition(&mut self, context: &Context, item: &Item, content: &str) {
        if let Item::Trait(definition) = item {
            let trait_node = self.insert_definition(
                context,
                definition.ident.unraw().to_string(),
                DefinitionKind::Trait,
                definition.span(),
                &definition.attrs,
                content,
                None,
                visibility_kind(&definition.vis),
            );
            if let Some(parent) = trait_node {
                for associated in &definition.items {
                    let (name, attrs, span) = match associated {
                        syn::TraitItem::Fn(item) => {
                            (format!("fn {}", item.sig.ident), &item.attrs, item.span())
                        }
                        syn::TraitItem::Const(item) => {
                            (format!("const {}", item.ident), &item.attrs, item.span())
                        }
                        syn::TraitItem::Type(item) => {
                            (format!("type {}", item.ident), &item.attrs, item.span())
                        }
                        _ => continue,
                    };
                    self.insert_definition(
                        context,
                        name,
                        DefinitionKind::AssociatedItem,
                        span,
                        attrs,
                        content,
                        Some(parent.clone()),
                        DefinitionVisibility::Restricted,
                    );
                }
            }
            return;
        }
        let span = item.span();
        let (name, kind, attrs) = match item {
            Item::Fn(v) => (
                v.sig.ident.unraw().to_string(),
                DefinitionKind::Function,
                &v.attrs,
            ),
            Item::Struct(v) => (
                v.ident.unraw().to_string(),
                DefinitionKind::Struct,
                &v.attrs,
            ),
            Item::Enum(v) => (v.ident.unraw().to_string(), DefinitionKind::Enum, &v.attrs),
            Item::Union(v) => (v.ident.unraw().to_string(), DefinitionKind::Union, &v.attrs),
            Item::Type(v) => (
                v.ident.unraw().to_string(),
                DefinitionKind::TypeAlias,
                &v.attrs,
            ),
            Item::Const(v) => (
                v.ident.unraw().to_string(),
                DefinitionKind::Constant,
                &v.attrs,
            ),
            Item::Static(v) => (
                v.ident.unraw().to_string(),
                DefinitionKind::Static,
                &v.attrs,
            ),
            Item::Macro(v) => match &v.ident {
                Some(ident) => (ident.unraw().to_string(), DefinitionKind::Macro, &v.attrs),
                None => return,
            },
            Item::Impl(v) => {
                let definition = self.insert_definition(
                    context,
                    "impl".into(),
                    DefinitionKind::Implementation,
                    span,
                    &v.attrs,
                    content,
                    None,
                    DefinitionVisibility::Restricted,
                );
                if let Some(impl_id) = definition {
                    for associated in &v.items {
                        let (name, attrs, span) = match associated {
                            ImplItem::Fn(item) => {
                                (format!("fn {}", item.sig.ident), &item.attrs, item.span())
                            }
                            ImplItem::Const(item) => {
                                (format!("const {}", item.ident), &item.attrs, item.span())
                            }
                            ImplItem::Type(item) => {
                                (format!("type {}", item.ident), &item.attrs, item.span())
                            }
                            _ => continue,
                        };
                        self.insert_definition(
                            context,
                            name,
                            DefinitionKind::AssociatedItem,
                            span,
                            attrs,
                            content,
                            Some(impl_id.clone()),
                            DefinitionVisibility::Restricted,
                        );
                    }
                }
                return;
            }
            _ => return,
        };
        self.insert_definition(
            context,
            name,
            kind,
            span,
            attrs,
            content,
            None,
            item_visibility(item),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_definition(
        &mut self,
        context: &Context,
        name: String,
        kind: DefinitionKind,
        span: Span,
        attrs: &[Attribute],
        content: &str,
        parent: Option<NodeId>,
        visibility: DefinitionVisibility,
    ) -> Option<NodeId> {
        let source_span = span_to_source(span, content);
        let condition = match condition_from_attributes(attrs) {
            Ok(own) => combine_conditions(context.inherited_condition.clone(), own),
            Err(()) => {
                self.report(
                    "CFG_EXPRESSION_UNSUPPORTED",
                    Severity::Warning,
                    "definition cfg expression could not be normalized",
                    Some(context.source_rel.clone()),
                    Some(source_span.clone()),
                );
                self.status = AnalysisStatus::Partial;
                combine_conditions(
                    context.inherited_condition.clone(),
                    Some(RelationshipCondition::Unknown),
                )
            }
        };
        let id = DefinitionId::from(format!(
            "{}::definition:{}:{}:{}",
            self.crate_id.0,
            kind_key(kind),
            name,
            source_span.start_byte
        ));
        let node_id = NodeId::Definition(id.clone());
        let node = Node::Definition(DefinitionNode {
            id,
            module: context.module_id.clone(),
            name: name.clone(),
            kind,
            visibility,
        });
        if let Err(error) = self.builder.add_node(node) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                Some(context.source_rel.clone()),
                Some(source_span),
            );
            self.status = AnalysisStatus::Partial;
            return None;
        }
        let evidence = Evidence {
            kind: EvidenceKind::DefinitionDeclaration,
            provenance: Provenance::RustSyntax,
            path: Some(context.source_rel.clone()),
            span: Some(source_span.clone()),
        };
        let owner = parent
            .clone()
            .unwrap_or_else(|| NodeId::Module(context.module_id.clone()));
        if let Err(error) = self.builder.add_edge(module_edge(
            EdgeKind::Contains,
            owner,
            Some(node_id.clone()),
            condition.clone(),
            evidence,
            None,
        )) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                Some(context.source_rel.clone()),
                Some(source_span),
            );
            self.status = AnalysisStatus::Partial;
        }
        if parent.is_none() {
            self.definitions
                .entry((context.logical_path.clone(), name.clone()))
                .or_default()
                .push((node_id.clone(), condition, visibility));
        }
        Some(node_id)
    }

    fn collect_import(&mut self, context: &Context, item: &syn::ItemUse, content: &str) {
        let own_condition = match condition_from_attributes(&item.attrs) {
            Ok(value) => value,
            Err(()) => {
                self.report(
                    "CFG_EXPRESSION_UNSUPPORTED",
                    Severity::Warning,
                    "import cfg expression could not be normalized",
                    Some(context.source_rel.clone()),
                    Some(span_to_source(item.span(), content)),
                );
                self.status = AnalysisStatus::Partial;
                Some(RelationshipCondition::Unknown)
            }
        };
        let condition = combine_conditions(context.inherited_condition.clone(), own_condition);
        let evidence = Evidence {
            kind: if !matches!(item.vis, syn::Visibility::Inherited) {
                EvidenceKind::Reexport
            } else {
                EvidenceKind::Import
            },
            provenance: Provenance::RustSyntax,
            path: Some(context.source_rel.clone()),
            span: Some(span_to_source(item.span(), content)),
        };
        let reexport = !matches!(item.vis, syn::Visibility::Inherited);
        let mut leaves = Vec::new();
        flatten_use_tree(&item.tree, Vec::new(), &mut leaves);
        for (path, alias, glob, self_leaf) in leaves {
            let spelling = format!(
                "{}{}{}{}",
                if item.leading_colon.is_some() {
                    "::"
                } else {
                    ""
                },
                path.join("::"),
                if self_leaf {
                    "::self"
                } else if glob {
                    "::*"
                } else {
                    ""
                },
                alias
                    .as_ref()
                    .map(|v| format!(" as {v}"))
                    .unwrap_or_default()
            );
            self.pending_imports.push(PendingImport {
                module: context.module_id.clone(),
                logical_module: context.logical_path.clone(),
                path,
                absolute: item.leading_colon.is_some(),
                spelling,
                alias,
                reexport,
                condition: condition.clone(),
                evidence: evidence.clone(),
                edition: self.edition.clone(),
            });
        }
    }

    fn resolve_imports(&mut self) {
        let mut aliases = AliasResolution::default();
        for import in &self.pending_imports {
            if let Some(alias) = &import.alias {
                aliases
                    .declared
                    .insert((import.logical_module.clone(), alias.clone()));
            }
        }
        for _ in 0..self.pending_imports.len() {
            for import in self.pending_imports.clone() {
                if let Some(alias) = &import.alias
                    && let Some(target) = self.resolve_import_target(&import, &aliases)
                    && matches!(target, NodeId::Module(_))
                    && self.target_condition(&target, &import).is_ok()
                {
                    let key = (import.logical_module.clone(), alias.clone());
                    let resolved = node_logical_path(&target, &self.modules);
                    if aliases
                        .resolved
                        .get(&key)
                        .is_some_and(|previous| previous != &resolved)
                    {
                        aliases.resolved.remove(&key);
                        if aliases.ambiguous.insert(key.clone()) {
                            self.report(
                                "IMPORT_ALIAS_AMBIGUOUS",
                                Severity::Warning,
                                format!(
                                    "module alias '{}' refers to multiple local modules",
                                    alias
                                ),
                                import.evidence.path.clone(),
                                import.evidence.span.clone(),
                            );
                            self.dependency_status = AnalysisStatus::Partial;
                        }
                    } else if !aliases.ambiguous.contains(&key) {
                        aliases.resolved.insert(key, resolved);
                    }
                }
            }
        }
        for import in self.pending_imports.clone() {
            let target = self.resolve_import_target(&import, &aliases);
            let edge_kind = if import.reexport {
                EdgeKind::Reexports
            } else {
                EdgeKind::Imports
            };
            let evidence = import.evidence.clone();
            let syntax = import.spelling.clone();
            if let Some(target) = target {
                let condition = match self.target_condition(&target, &import) {
                    Ok(condition) => condition,
                    Err(()) => {
                        self.unresolved_import(
                            &import,
                            "CONDITIONAL_RESOLUTION_AMBIGUOUS",
                            "target differs between configuration alternatives",
                        );
                        continue;
                    }
                };
                let mut edge = module_edge(
                    edge_kind,
                    NodeId::Module(import.module.clone()),
                    Some(target.clone()),
                    condition.clone(),
                    evidence.clone(),
                    None,
                );
                edge.resolution = import_resolution(&condition);
                set_edge_syntax(&mut edge, syntax.clone());
                if let Err(error) = self.builder.add_edge(edge) {
                    self.report(
                        "GRAPH_CONSTRUCTION_FAILED",
                        Severity::Error,
                        error.to_string(),
                        evidence.path.clone(),
                        evidence.span.clone(),
                    );
                    self.status = AnalysisStatus::Partial;
                }
                if matches!(target, NodeId::Module(_)) {
                    let mut reference = module_edge(
                        EdgeKind::ReferencesModule,
                        NodeId::Module(import.module.clone()),
                        Some(target),
                        condition.clone(),
                        evidence.clone(),
                        None,
                    );
                    reference.resolution = import_resolution(&condition);
                    reference.evidence[0].kind = EvidenceKind::ModuleReference;
                    set_edge_syntax(&mut reference, syntax);
                    if let Err(error) = self.builder.add_edge(reference) {
                        self.report(
                            "GRAPH_CONSTRUCTION_FAILED",
                            Severity::Error,
                            error.to_string(),
                            evidence.path.clone(),
                            evidence.span.clone(),
                        );
                        self.status = AnalysisStatus::Partial;
                    }
                }
            } else {
                let external = import.path.first().is_some_and(|name| {
                    self.external_crates.contains(name)
                        || matches!(name.as_str(), "std" | "core" | "alloc")
                });
                let alias_ambiguity = import.path.first().is_some_and(|name| {
                    aliases
                        .ambiguous
                        .contains(&(import.logical_module.clone(), name.clone()))
                });
                let conditional_ambiguity =
                    !external && self.ambiguous_conditional_target(&import, &aliases);
                self.unresolved_import(
                    &import,
                    if alias_ambiguity {
                        "IMPORT_ALIAS_AMBIGUOUS"
                    } else if conditional_ambiguity {
                        "CONDITIONAL_RESOLUTION_AMBIGUOUS"
                    } else if external {
                        "CROSS_CRATE_RESOLUTION_UNSUPPORTED"
                    } else {
                        "IMPORT_UNRESOLVED"
                    },
                    if alias_ambiguity {
                        "an earlier use alias has multiple possible module targets"
                    } else if conditional_ambiguity {
                        "multiple conditional definitions match this path"
                    } else if external {
                        "external target is outside local module resolution"
                    } else {
                        "path did not match a uniquely discovered local definition or module"
                    },
                );
            }
        }
    }

    fn unresolved_import(&mut self, import: &PendingImport, code: &'static str, message: &str) {
        let evidence = import.evidence.clone();
        let mut edge = module_edge(
            EdgeKind::UnresolvedRelationship,
            NodeId::Module(import.module.clone()),
            None,
            import.condition.clone(),
            evidence.clone(),
            Some(format!("use {}", import.spelling)),
        );
        set_edge_syntax(&mut edge, import.spelling.clone());
        if let Err(error) = self.builder.add_edge(edge) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                evidence.path.clone(),
                evidence.span.clone(),
            );
        }
        let mut diagnostic = diagnostic(
            code,
            if code == "CROSS_CRATE_RESOLUTION_UNSUPPORTED" {
                Severity::Info
            } else {
                Severity::Warning
            },
            DiagnosticPhase::Parsing,
            format!("import '{}': {message}", import.spelling),
            evidence.path.clone(),
            None,
        );
        diagnostic
            .related_nodes
            .push(NodeId::Module(import.module.clone()));
        self.builder.add_diagnostic(diagnostic);
        self.dependency_status = AnalysisStatus::Partial;
    }

    fn target_condition(
        &self,
        target: &NodeId,
        import: &PendingImport,
    ) -> Result<Option<RelationshipCondition>, ()> {
        let found_condition = match target {
            NodeId::Module(id) => {
                let path = self
                    .modules
                    .iter()
                    .find_map(|(path, candidate)| (candidate == id).then(|| path.clone()))
                    .ok_or(())?;
                let conditions = self.module_conditions.get(&path).ok_or(())?;
                if conditions.len() > 1 {
                    return Err(());
                }
                conditions.first().cloned().flatten()
            }
            NodeId::Definition(_) => {
                let matches: Vec<_> = self
                    .definitions
                    .values()
                    .flat_map(|items| items.iter().filter(|(node, _, _)| node == target))
                    .collect();
                if matches.len() != 1 {
                    return Err(());
                }
                matches[0].1.clone()
            }
            _ => None,
        };
        Ok(combine_conditions(
            import.condition.clone(),
            found_condition,
        ))
    }

    fn module_path_accessible(&self, logical_path: &str, importer: &str) -> bool {
        if logical_path == "crate" {
            return true;
        }
        let mut prefix = "crate".to_owned();
        for segment in logical_path
            .strip_prefix("crate::")
            .unwrap_or_default()
            .split("::")
        {
            let parent = prefix.clone();
            prefix.push_str("::");
            prefix.push_str(segment);
            let Some(visibilities) = self.module_visibility.get(&prefix) else {
                return false;
            };
            if visibilities.len() != 1 {
                return false;
            }
            match visibilities
                .first()
                .copied()
                .unwrap_or(DefinitionVisibility::Restricted)
            {
                DefinitionVisibility::Public | DefinitionVisibility::Crate => {}
                DefinitionVisibility::Private
                    if importer == parent || importer.starts_with(&format!("{parent}::")) => {}
                DefinitionVisibility::Private | DefinitionVisibility::Restricted => return false,
            }
        }
        true
    }

    fn ambiguous_conditional_target(
        &self,
        import: &PendingImport,
        aliases: &AliasResolution,
    ) -> bool {
        if import.absolute {
            return false;
        }
        let mut parts = import.path.clone();
        let Some(first) = parts.first().cloned() else {
            return false;
        };
        let base = match first.as_str() {
            "crate" => {
                parts.remove(0);
                "crate".to_owned()
            }
            "self" => {
                parts.remove(0);
                import.logical_module.clone()
            }
            "super" => {
                let mut parent = import.logical_module.clone();
                while parts.first().is_some_and(|part| part == "super") {
                    parts.remove(0);
                    parent = parent
                        .rsplit_once("::")
                        .map(|(p, _)| p.to_owned())
                        .unwrap_or_else(|| "crate".into());
                }
                parent
            }
            _ if aliases
                .ambiguous
                .contains(&(import.logical_module.clone(), first.clone()))
                || (aliases
                    .declared
                    .contains(&(import.logical_module.clone(), first.clone()))
                    && !aliases
                        .resolved
                        .contains_key(&(import.logical_module.clone(), first.clone()))) =>
            {
                return false;
            }
            _ if aliases
                .resolved
                .contains_key(&(import.logical_module.clone(), first.clone())) =>
            {
                parts.remove(0);
                aliases.resolved[&(import.logical_module.clone(), first)].clone()
            }
            _ => "crate".to_owned(),
        };
        let Some(last) = parts.pop() else {
            return false;
        };
        let parent = if parts.is_empty() {
            base
        } else {
            format!("{base}::{}", parts.join("::"))
        };
        self.definitions
            .get(&(parent, last))
            .is_some_and(|candidates| candidates.len() > 1)
    }

    fn resolve_import_target(
        &self,
        import: &PendingImport,
        aliases: &AliasResolution,
    ) -> Option<NodeId> {
        if import.absolute {
            return None;
        }
        let mut parts = import.path.clone();
        if parts.is_empty() {
            return None;
        }
        let current = import.logical_module.clone();
        let base = match parts.first()?.as_str() {
            "crate" => {
                parts.remove(0);
                "crate".to_owned()
            }
            "self" => {
                parts.remove(0);
                current
            }
            "super" => {
                let mut parent = current;
                while parts.first().is_some_and(|part| part == "super") {
                    if parent == "crate" {
                        return None;
                    }
                    parts.remove(0);
                    parent = parent
                        .rsplit_once("::")
                        .map(|(p, _)| p.to_owned())
                        .unwrap_or_else(|| "crate".into());
                }
                parent
            }
            first => {
                let alias_key = (current.clone(), first.to_owned());
                if aliases.ambiguous.contains(&alias_key)
                    || (aliases.declared.contains(&alias_key)
                        && !aliases.resolved.contains_key(&alias_key))
                {
                    return None;
                } else if let Some(alias_target) = aliases.resolved.get(&alias_key) {
                    parts.remove(0);
                    alias_target.clone()
                } else if self.external_crates.contains(first)
                    || matches!(first, "std" | "core" | "alloc")
                {
                    return None;
                } else {
                    // Edition 2015/2018+ use paths can be crate-root anchored;
                    // only accept a root module path already proven by a mod declaration.
                    if matches!(import.edition.as_str(), "2015" | "2018" | "2021" | "2024") {
                        "crate".to_owned()
                    } else {
                        return None;
                    }
                }
            }
        };
        let full = if parts.is_empty() {
            base.clone()
        } else {
            format!("{base}::{}", parts.join("::"))
        };
        if let Some(module) = self.modules.get(&full) {
            if !self.module_path_accessible(&full, &import.logical_module) {
                return None;
            }
            return Some(NodeId::Module(module.clone()));
        }
        if !parts.is_empty() {
            let parent = if parts.len() == 1 {
                base.clone()
            } else {
                format!("{base}::{}", parts[..parts.len() - 1].join("::"))
            };
            if self.module_path_accessible(&parent, &import.logical_module)
                && let Some(definitions) = self
                    .definitions
                    .get(&(parent.clone(), parts.last()?.clone()))
                && definitions.len() == 1
            {
                let (target, _, visibility) = &definitions[0];
                let local_access = import.logical_module == parent
                    || import.logical_module.starts_with(&format!("{parent}::"));
                if matches!(
                    visibility,
                    DefinitionVisibility::Public | DefinitionVisibility::Crate
                ) || local_access
                {
                    return Some(target.clone());
                }
            }
        }
        None
    }

    fn process_module(&mut self, context: &Context, module: &ItemMod, content: &str) {
        let name = module.ident.unraw().to_string();
        let logical_path = format!("{}::{name}", context.logical_path);
        let child_id = ModuleId::from(format!("{}::module:{logical_path}", self.crate_id.0));
        let own_condition = match condition_from_attributes(&module.attrs) {
            Ok(condition) => condition,
            Err(()) => {
                self.report(
                    "CFG_EXPRESSION_UNSUPPORTED",
                    Severity::Warning,
                    "module cfg expression could not be normalized",
                    Some(context.source_rel.clone()),
                    Some(span_to_source(module.span(), content)),
                );
                self.status = AnalysisStatus::Partial;
                Some(RelationshipCondition::Unknown)
            }
        };
        let unsupported_attributes = unsupported_module_attributes(&module.attrs);
        let mut condition = combine_conditions(context.inherited_condition.clone(), own_condition);
        if !unsupported_attributes.is_empty() {
            condition = combine_conditions(condition, Some(RelationshipCondition::Unknown));
        }
        for attribute in &unsupported_attributes {
            self.report("MODULE_ATTRIBUTE_UNSUPPORTED", Severity::Warning, format!("module attribute '{attribute}' is not interpreted and may alter module structure"), Some(context.source_rel.clone()), Some(span_to_source(module.span(), content)));
            self.status = AnalysisStatus::Partial;
        }
        let declaration_evidence = Evidence {
            kind: EvidenceKind::ModuleDeclaration,
            provenance: Provenance::RustSyntax,
            path: Some(context.source_rel.clone()),
            span: Some(span_to_source(module.span(), content)),
        };
        let child_node = Node::Module(ModuleNode {
            id: child_id.clone(),
            crate_id: self.crate_id.clone(),
            logical_path: logical_path.clone(),
        });
        if let Err(error) = self.builder.add_node(child_node) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                Some(context.source_rel.clone()),
                Some(span_to_source(module.span(), content)),
            );
            self.status = AnalysisStatus::Partial;
            return;
        }
        self.modules.insert(logical_path.clone(), child_id.clone());
        self.module_conditions
            .entry(logical_path.clone())
            .or_default()
            .insert(condition.clone());
        self.module_visibility
            .entry(logical_path.clone())
            .or_default()
            .insert(visibility_kind(&module.vis));
        let parent_id = NodeId::Module(context.module_id.clone());
        let child_node_id = NodeId::Module(child_id.clone());
        for (kind, evidence) in [
            (EdgeKind::Contains, declaration_evidence.clone()),
            (EdgeKind::DeclaresModule, declaration_evidence.clone()),
        ] {
            if let Err(error) = self.builder.add_edge(module_edge(
                kind,
                parent_id.clone(),
                Some(child_node_id.clone()),
                condition.clone(),
                evidence,
                None,
            )) {
                self.report(
                    "GRAPH_CONSTRUCTION_FAILED",
                    Severity::Error,
                    error.to_string(),
                    Some(context.source_rel.clone()),
                    Some(span_to_source(module.span(), content)),
                );
                self.status = AnalysisStatus::Partial;
            }
        }

        if !unsupported_attributes.is_empty() {
            if module.content.is_some() {
                self.map_inline_module(
                    &child_id,
                    &context.file_rel,
                    condition,
                    declaration_evidence,
                );
            } else {
                self.unresolved_module(
                    &parent_id,
                    &name,
                    context,
                    module,
                    content,
                    condition,
                    declaration_evidence,
                );
            }
            return;
        }

        if let Some((target, nested)) = &module.content {
            let (inline_dir, module_dir_ambiguous) =
                match inline_module_dir(self.root, context, &name, module) {
                    Ok(path) => path,
                    Err(message) => {
                        self.report(
                            "MODULE_PATH_UNSUPPORTED",
                            Severity::Warning,
                            message,
                            Some(context.source_rel.clone()),
                            Some(span_to_source(module.span(), content)),
                        );
                        self.status = AnalysisStatus::Partial;
                        (context.module_dir.join(&name), true)
                    }
                };
            self.map_inline_module(
                &child_id,
                &context.file_rel,
                condition.clone(),
                declaration_evidence.clone(),
            );
            let child_context = Context {
                module_id: child_id,
                logical_path,
                source: context.source.clone(),
                resolution_path: context.resolution_path.clone(),
                source_rel: context.source_rel.clone(),
                file_rel: context.file_rel.clone(),
                module_dir: inline_dir,
                inline_depth: context.inline_depth + 1,
                inherited_condition: condition,
                module_dir_ambiguous: context.module_dir_ambiguous || module_dir_ambiguous,
            };
            self.process_items(&child_context, nested, content);
            let _ = target;
            return;
        }

        let path_attribute = match module_path_attribute(&module.attrs) {
            Ok(result) => result,
            Err(message) => {
                self.unresolved_module(
                    &parent_id,
                    &name,
                    context,
                    module,
                    content,
                    condition,
                    declaration_evidence,
                );
                self.report(
                    "MODULE_PATH_UNSUPPORTED",
                    Severity::Warning,
                    message,
                    Some(context.source_rel.clone()),
                    Some(span_to_source(module.span(), content)),
                );
                self.status = AnalysisStatus::Partial;
                return;
            }
        };
        if path_attribute.conditional {
            self.unresolved_module(
                &parent_id,
                &name,
                context,
                module,
                content,
                condition,
                declaration_evidence,
            );
            self.report("CONDITIONAL_MODULE_PATH_AMBIGUOUS", Severity::Warning, "cfg_attr changes this module's source path; Atlas does not evaluate configuration and leaves the source unresolved", Some(context.source_rel.clone()), Some(span_to_source(module.span(), content)));
            self.status = AnalysisStatus::Partial;
            return;
        }
        if context.module_dir_ambiguous {
            self.unresolved_module(
                &parent_id,
                &name,
                context,
                module,
                content,
                condition,
                declaration_evidence,
            );
            self.report("CONDITIONAL_MODULE_PATH_AMBIGUOUS", Severity::Warning, "an ancestor cfg_attr changes the module directory, so this child source cannot be selected safely", Some(context.source_rel.clone()), Some(span_to_source(module.span(), content)));
            self.status = AnalysisStatus::Partial;
            return;
        }
        let explicit_path = path_attribute.path.is_some();
        let candidates = if let Some(path) = path_attribute.path {
            let base = if context.inline_depth == 0 {
                context.resolution_path.parent().unwrap_or(self.root)
            } else {
                &context.module_dir
            };
            match normalize_path(base, Path::new(&path), self.root) {
                Ok(path) => vec![path],
                Err(message) => {
                    self.unresolved_module(
                        &parent_id,
                        &name,
                        context,
                        module,
                        content,
                        condition,
                        declaration_evidence,
                    );
                    self.report(
                        "MODULE_PATH_OUT_OF_BOUNDS",
                        Severity::Error,
                        message,
                        Some(context.source_rel.clone()),
                        Some(span_to_source(module.span(), content)),
                    );
                    self.status = AnalysisStatus::Partial;
                    return;
                }
            }
        } else {
            let Some(base) = normalize_path(&context.module_dir, Path::new(&name), self.root).ok()
            else {
                self.unresolved_module(
                    &parent_id,
                    &name,
                    context,
                    module,
                    content,
                    condition,
                    declaration_evidence,
                );
                self.report(
                    "MODULE_PATH_OUT_OF_BOUNDS",
                    Severity::Error,
                    "module path escapes the selected repository boundary",
                    Some(context.source_rel.clone()),
                    Some(span_to_source(module.span(), content)),
                );
                self.status = AnalysisStatus::Partial;
                return;
            };
            vec![base.with_extension("rs"), base.join("mod.rs")]
        };
        match self.choose_candidate(&candidates, &name, context, module, content) {
            Some(source) => {
                let canonical = match canonical_source(self.root, &source) {
                    Ok(path) => path,
                    Err((code, message)) => {
                        self.unresolved_module(
                            &parent_id,
                            &name,
                            context,
                            module,
                            content,
                            condition,
                            declaration_evidence,
                        );
                        self.report(
                            code,
                            Severity::Error,
                            message,
                            Some(context.source_rel.clone()),
                            Some(span_to_source(module.span(), content)),
                        );
                        self.status = AnalysisStatus::Partial;
                        return;
                    }
                };
                let file_rel = match relative_to(self.root, &canonical) {
                    Ok(path) => path,
                    Err(message) => {
                        self.unresolved_module(
                            &parent_id,
                            &name,
                            context,
                            module,
                            content,
                            condition,
                            declaration_evidence,
                        );
                        self.report(
                            "MODULE_PATH_OUT_OF_BOUNDS",
                            Severity::Error,
                            message,
                            Some(context.source_rel.clone()),
                            Some(span_to_source(module.span(), content)),
                        );
                        self.status = AnalysisStatus::Partial;
                        return;
                    }
                };
                let source_rel = match relative_lexical_to(self.root, &source) {
                    Ok(path) => path,
                    Err(message) => {
                        self.unresolved_module(
                            &parent_id,
                            &name,
                            context,
                            module,
                            content,
                            condition,
                            declaration_evidence,
                        );
                        self.report(
                            "MODULE_PATH_OUT_OF_BOUNDS",
                            Severity::Error,
                            message,
                            Some(context.source_rel.clone()),
                            Some(span_to_source(module.span(), content)),
                        );
                        self.status = AnalysisStatus::Partial;
                        return;
                    }
                };
                let edge = module_edge(
                    EdgeKind::LoadsSource,
                    NodeId::Module(child_id.clone()),
                    Some(NodeId::SourceFile(crate::model::SourceFileId::from(
                        format!("file:{}", file_rel.as_str()),
                    ))),
                    condition.clone(),
                    declaration_evidence,
                    None,
                );
                let child_context = Context {
                    module_id: child_id,
                    logical_path,
                    source: canonical.clone(),
                    resolution_path: source.clone(),
                    source_rel,
                    file_rel,
                    module_dir: if explicit_path {
                        source.parent().unwrap_or(self.root).to_path_buf()
                    } else {
                        module_dir_for_external(&source, &name)
                    },
                    inline_depth: 0,
                    inherited_condition: condition.clone(),
                    module_dir_ambiguous: false,
                };
                self.walk_file(child_context, Some(edge));
            }
            None => self.unresolved_module(
                &parent_id,
                &name,
                context,
                module,
                content,
                condition,
                declaration_evidence,
            ),
        }
    }

    fn map_inline_module(
        &mut self,
        module_id: &ModuleId,
        source: &RepoPath,
        condition: Option<RelationshipCondition>,
        evidence: Evidence,
    ) {
        let source_id = source_file_node(source.clone());
        if let Err(error) = self.builder.add_node(source_id.clone()) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                Some(source.clone()),
                None,
            );
            self.status = AnalysisStatus::Partial;
            return;
        }
        if let Err(error) = self.builder.add_edge(module_edge(
            EdgeKind::LoadsSource,
            NodeId::Module(module_id.clone()),
            Some(source_id.id()),
            condition,
            evidence,
            None,
        )) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                Some(source.clone()),
                None,
            );
            self.status = AnalysisStatus::Partial;
        }
    }

    fn choose_candidate(
        &mut self,
        candidates: &[PathBuf],
        name: &str,
        context: &Context,
        module: &ItemMod,
        content: &str,
    ) -> Option<PathBuf> {
        let mut existing = Vec::new();
        for candidate in candidates {
            match fs::symlink_metadata(candidate) {
                Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                    existing.push(candidate.clone())
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.report(
                        "SOURCE_INACCESSIBLE",
                        Severity::Error,
                        format!("cannot inspect module candidate ({})", error.kind()),
                        Some(context.source_rel.clone()),
                        Some(span_to_source(module.span(), content)),
                    );
                    self.status = AnalysisStatus::Partial;
                }
            }
        }
        if existing.len() > 1 {
            self.report("MODULE_FILE_CONFLICT", Severity::Error, format!("both conventional source candidates for module '{name}' exist; Atlas will not choose between them"), Some(context.source_rel.clone()), Some(span_to_source(module.span(), content)));
            self.status = AnalysisStatus::Partial;
            return None;
        }
        if existing.is_empty() {
            self.report(
                "MODULE_FILE_MISSING",
                Severity::Error,
                format!("no source file was found for declared module '{name}'"),
                Some(context.source_rel.clone()),
                Some(span_to_source(module.span(), content)),
            );
            self.status = AnalysisStatus::Partial;
            return None;
        }
        existing.pop()
    }

    #[allow(clippy::too_many_arguments)] // Carries the parsed item, context, span source, and graph condition.
    fn unresolved_module(
        &mut self,
        parent: &NodeId,
        name: &str,
        context: &Context,
        module: &ItemMod,
        content: &str,
        condition: Option<RelationshipCondition>,
        mut evidence: Evidence,
    ) {
        evidence.kind = EvidenceKind::ModuleDeclaration;
        let edge = module_edge(
            EdgeKind::UnresolvedRelationship,
            parent.clone(),
            None,
            condition,
            evidence,
            Some(format!("module {name}")),
        );
        if let Err(error) = self.builder.add_edge(edge) {
            self.report(
                "GRAPH_CONSTRUCTION_FAILED",
                Severity::Error,
                error.to_string(),
                Some(context.source_rel.clone()),
                Some(span_to_source(module.span(), content)),
            );
            self.status = AnalysisStatus::Partial;
        }
    }

    fn report(
        &mut self,
        code: &'static str,
        severity: Severity,
        message: impl Into<String>,
        path: Option<RepoPath>,
        span: Option<SourceSpan>,
    ) {
        self.builder.add_diagnostic(diagnostic(
            code,
            severity,
            DiagnosticPhase::Parsing,
            message,
            path,
            span,
        ));
    }
}

fn import_resolution(condition: &Option<RelationshipCondition>) -> ResolutionStatus {
    fn has_unknown(condition: &RelationshipCondition) -> bool {
        match condition {
            RelationshipCondition::Unknown => true,
            RelationshipCondition::All(items) | RelationshipCondition::Any(items) => {
                items.iter().any(has_unknown)
            }
            RelationshipCondition::Not(inner) => has_unknown(inner),
            _ => false,
        }
    }
    match condition {
        Some(condition) if has_unknown(condition) => ResolutionStatus::Unsupported,
        Some(_) => ResolutionStatus::Conditional,
        None => ResolutionStatus::Resolved,
    }
}

fn kind_key(kind: DefinitionKind) -> &'static str {
    match kind {
        DefinitionKind::Function => "function",
        DefinitionKind::Struct => "struct",
        DefinitionKind::Enum => "enum",
        DefinitionKind::Union => "union",
        DefinitionKind::Trait => "trait",
        DefinitionKind::TypeAlias => "type_alias",
        DefinitionKind::Constant => "constant",
        DefinitionKind::Static => "static",
        DefinitionKind::Macro => "macro",
        DefinitionKind::Implementation => "implementation",
        DefinitionKind::AssociatedItem => "associated_item",
        DefinitionKind::Other => "other",
    }
}

fn visibility_kind(visibility: &syn::Visibility) -> DefinitionVisibility {
    match visibility {
        syn::Visibility::Public(_) => DefinitionVisibility::Public,
        syn::Visibility::Restricted(restricted) if restricted.path.is_ident("crate") => {
            DefinitionVisibility::Crate
        }
        syn::Visibility::Restricted(_) => DefinitionVisibility::Restricted,
        syn::Visibility::Inherited => DefinitionVisibility::Private,
    }
}

fn item_visibility(item: &Item) -> DefinitionVisibility {
    let visibility = match item {
        Item::Fn(v) => &v.vis,
        Item::Struct(v) => &v.vis,
        Item::Enum(v) => &v.vis,
        Item::Union(v) => &v.vis,
        Item::Type(v) => &v.vis,
        Item::Const(v) => &v.vis,
        Item::Static(v) => &v.vis,
        Item::Macro(v) => {
            return if v
                .attrs
                .iter()
                .any(|attribute| attribute.path().is_ident("macro_export"))
            {
                DefinitionVisibility::Public
            } else {
                DefinitionVisibility::Private
            };
        }
        _ => return DefinitionVisibility::Restricted,
    };
    visibility_kind(visibility)
}

fn flatten_use_tree(
    tree: &UseTree,
    prefix: Vec<String>,
    out: &mut Vec<(Vec<String>, Option<String>, bool, bool)>,
) {
    match tree {
        UseTree::Path(path) => {
            let mut next = prefix;
            next.push(path.ident.unraw().to_string());
            flatten_use_tree(&path.tree, next, out);
        }
        UseTree::Name(name) => {
            let name = name.ident.unraw().to_string();
            if name == "self" && !prefix.is_empty() {
                out.push((prefix, None, false, true));
            } else {
                let mut path = prefix;
                path.push(name);
                out.push((path, None, false, false));
            }
        }
        UseTree::Rename(rename) => {
            let name = rename.ident.unraw().to_string();
            let alias = rename.rename.unraw().to_string();
            if name == "self" && !prefix.is_empty() {
                out.push((prefix, Some(alias), false, true));
            } else {
                let mut path = prefix;
                path.push(name);
                out.push((path, Some(alias), false, false));
            }
        }
        UseTree::Glob(_) => out.push((prefix, None, true, false)),
        UseTree::Group(group) => {
            for item in &group.items {
                flatten_use_tree(item, prefix.clone(), out);
            }
        }
    }
}

fn set_edge_syntax(edge: &mut crate::model::Edge, syntax: String) {
    edge.syntax = Some(syntax.clone());
    edge.id = crate::model::EdgeId::from(format!("{}::syntax:{syntax}", edge.id.0));
}

fn node_logical_path(node: &NodeId, modules: &BTreeMap<String, ModuleId>) -> String {
    match node {
        NodeId::Module(id) => modules
            .iter()
            .find_map(|(path, candidate)| (candidate == id).then(|| path.clone()))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

struct PathAttribute {
    path: Option<String>,
    conditional: bool,
}

fn module_path_attribute(attrs: &[Attribute]) -> Result<PathAttribute, String> {
    let mut path = None;
    let mut conditional = false;
    for attr in attrs {
        if attr.path().is_ident("path") {
            match &attr.meta {
                Meta::NameValue(value) => match &value.value {
                    syn::Expr::Lit(expr) => match &expr.lit {
                        Lit::Str(value) => {
                            let value = value.value();
                            if path.as_ref().is_some_and(|previous| previous != &value) {
                                return Err("module has conflicting literal path attributes".into());
                            }
                            path = Some(value);
                        }
                        _ => {
                            return Err(
                                "module path attribute must contain a string literal".into()
                            );
                        }
                    },
                    _ => return Err("module path attribute must contain a string literal".into()),
                },
                _ => return Err("unsupported path attribute form".into()),
            }
        } else if attr.path().is_ident("cfg_attr") {
            let args = attr
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .map_err(|_| "cfg_attr could not be parsed".to_owned())?;
            for nested in args.iter().skip(1) {
                conditional |= contains_path_cfg_attr(nested);
            }
        }
    }
    Ok(PathAttribute { path, conditional })
}

fn inline_module_dir(
    root: &Path,
    context: &Context,
    name: &str,
    module: &ItemMod,
) -> Result<(PathBuf, bool), String> {
    let attribute = module_path_attribute(&module.attrs)?;
    let relative = attribute.path.unwrap_or_else(|| name.to_owned());
    let path = normalize_path(&context.module_dir, Path::new(&relative), root)?;
    Ok((path, attribute.conditional))
}

fn module_dir_for_external(source: &Path, module_name: &str) -> PathBuf {
    // Rust's default child-module directory is named after the module, even
    // when its source file uses the legacy `<name>/mod.rs` layout.
    let parent = source.parent().unwrap_or(Path::new("."));
    if source.file_name().is_some_and(|name| name == "mod.rs") {
        parent.to_path_buf()
    } else {
        parent.join(module_name)
    }
}

fn condition_from_attributes(attrs: &[Attribute]) -> Result<Option<RelationshipCondition>, ()> {
    let mut conditions = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("cfg") {
            let predicate = attr.parse_args::<Meta>().map_err(|_| ())?;
            conditions.push(parse_cfg_meta(&predicate).ok_or(())?);
        } else if attr.path().is_ident("cfg_attr") {
            let args = attr
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .map_err(|_| ())?;
            let Some(predicate) = args.first() else {
                return Err(());
            };
            let condition = parse_cfg_meta(predicate).ok_or(())?;
            for nested in args.iter().skip(1) {
                collect_cfg_attr_conditions(nested, vec![condition.clone()], &mut conditions)?;
            }
        }
    }
    Ok(match conditions.len() {
        0 => None,
        1 => conditions.pop(),
        _ => Some(RelationshipCondition::All(conditions)),
    })
}

fn collect_cfg_attr_conditions(
    meta: &Meta,
    gates: Vec<RelationshipCondition>,
    output: &mut Vec<RelationshipCondition>,
) -> Result<(), ()> {
    if meta.path().is_ident("cfg") {
        let Meta::List(list) = meta else {
            return Err(());
        };
        let predicate = Punctuated::<Meta, Token![,]>::parse_terminated
            .parse2(list.tokens.clone())
            .map_err(|_| ())?
            .into_iter()
            .next()
            .ok_or(())?;
        let predicate = parse_cfg_meta(&predicate).ok_or(())?;
        let mut alternatives: Vec<_> = gates
            .into_iter()
            .map(|gate| RelationshipCondition::Not(Box::new(gate)))
            .collect();
        alternatives.push(predicate);
        output.push(RelationshipCondition::Any(alternatives));
    } else if meta.path().is_ident("cfg_attr") {
        let Meta::List(list) = meta else {
            return Err(());
        };
        let args = Punctuated::<Meta, Token![,]>::parse_terminated
            .parse2(list.tokens.clone())
            .map_err(|_| ())?;
        let predicate = args.first().and_then(parse_cfg_meta).ok_or(())?;
        let mut nested_gates = gates;
        nested_gates.push(predicate);
        for nested in args.iter().skip(1) {
            collect_cfg_attr_conditions(nested, nested_gates.clone(), output)?;
        }
    }
    Ok(())
}

fn contains_path_cfg_attr(meta: &Meta) -> bool {
    if meta.path().is_ident("path") {
        return true;
    }
    if !meta.path().is_ident("cfg_attr") {
        return false;
    }
    let Meta::List(list) = meta else {
        return false;
    };
    Punctuated::<Meta, Token![,]>::parse_terminated
        .parse2(list.tokens.clone())
        .ok()
        .is_some_and(|args| args.iter().skip(1).any(contains_path_cfg_attr))
}

fn combine_conditions(
    parent: Option<RelationshipCondition>,
    own: Option<RelationshipCondition>,
) -> Option<RelationshipCondition> {
    match (parent, own) {
        (Some(parent), Some(own)) if parent == own => Some(parent),
        (Some(parent), Some(own)) => Some(RelationshipCondition::All(vec![parent, own])),
        (Some(condition), None) | (None, Some(condition)) => Some(condition),
        (None, None) => None,
    }
}

pub(crate) fn normalize_cfg_expression(input: &str) -> Option<RelationshipCondition> {
    let expression = input
        .strip_prefix("cfg(")
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(input);
    let meta = syn::parse_str::<Meta>(expression).ok()?;
    parse_cfg_meta(&meta)
}

fn unsupported_module_attributes(attrs: &[Attribute]) -> Vec<String> {
    let mut unsupported = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("cfg_attr") {
            if let Ok(args) = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            {
                for nested in args.iter().skip(1) {
                    collect_unsupported_meta(nested, &mut unsupported);
                }
            } else {
                unsupported.push("cfg_attr".into());
            }
        } else if !supported_attribute(attr.path()) {
            unsupported.push(path_to_string(attr.path()));
        }
    }
    unsupported.sort();
    unsupported.dedup();
    unsupported
}

fn unsupported_crate_attributes(attrs: &[Attribute]) -> Vec<String> {
    let known = [
        "cfg",
        "cfg_attr",
        "doc",
        "deprecated",
        "allow",
        "warn",
        "deny",
        "forbid",
        "no_std",
        "no_main",
        "no_builtins",
        "crate_name",
        "crate_type",
        "feature",
        "recursion_limit",
        "type_length_limit",
        "windows_subsystem",
        "panic_handler",
        "macro_use",
        "register_tool",
        "register_attr",
        "rustc_private",
        "rustc_deprecated",
    ];
    let mut unsupported: Vec<_> = attrs
        .iter()
        .filter(|attr| {
            attr.path()
                .get_ident()
                .is_none_or(|ident| !known.contains(&ident.to_string().as_str()))
        })
        .map(|attr| path_to_string(attr.path()))
        .collect();
    unsupported.sort();
    unsupported.dedup();
    unsupported
}

fn collect_unsupported_meta(meta: &Meta, unsupported: &mut Vec<String>) {
    if meta.path().is_ident("cfg_attr") {
        if let Meta::List(list) = meta
            && let Ok(args) =
                Punctuated::<Meta, Token![,]>::parse_terminated.parse2(list.tokens.clone())
        {
            for nested in args.iter().skip(1) {
                collect_unsupported_meta(nested, unsupported);
            }
            return;
        }
        unsupported.push("cfg_attr".into());
    } else if !supported_attribute(meta.path()) {
        unsupported.push(path_to_string(meta.path()));
    }
}

fn supported_attribute(path: &syn::Path) -> bool {
    path.get_ident().is_some_and(|ident| {
        matches!(
            ident.to_string().as_str(),
            "cfg"
                | "cfg_attr"
                | "path"
                | "doc"
                | "deprecated"
                | "allow"
                | "warn"
                | "deny"
                | "forbid"
                | "macro_use"
                | "no_implicit_prelude"
        )
    })
}

fn parse_cfg_meta(meta: &Meta) -> Option<RelationshipCondition> {
    match meta {
        Meta::Path(path) if path.is_ident("true") => Some(RelationshipCondition::Constant(true)),
        Meta::Path(path) if path.is_ident("false") => Some(RelationshipCondition::Constant(false)),
        Meta::Path(path) => Some(RelationshipCondition::Predicate {
            name: path_to_string(path),
            value: None,
        }),
        Meta::NameValue(value) => {
            let predicate_value = match &value.value {
                syn::Expr::Lit(expr) => match &expr.lit {
                    Lit::Str(v) => v.value(),
                    Lit::Bool(v) => v.value.to_string(),
                    _ => return None,
                },
                _ => return None,
            };
            Some(RelationshipCondition::Predicate {
                name: path_to_string(&value.path),
                value: Some(predicate_value),
            })
        }
        Meta::List(list)
            if list.path.is_ident("all")
                || list.path.is_ident("any")
                || list.path.is_ident("not") =>
        {
            let nested = Punctuated::<Meta, Token![,]>::parse_terminated
                .parse2(list.tokens.clone())
                .ok()?;
            let parsed: Option<Vec<_>> = nested.iter().map(parse_cfg_meta).collect();
            let parsed = parsed?;
            match list.path.get_ident()?.to_string().as_str() {
                "all" => Some(RelationshipCondition::All(parsed)),
                "any" => Some(RelationshipCondition::Any(parsed)),
                "not" if parsed.len() == 1 => Some(RelationshipCondition::Not(Box::new(
                    parsed.into_iter().next()?,
                ))),
                "not" => None,
                _ => None,
            }
        }
        _ => None,
    }
}

fn path_to_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn normalize_path(base: &Path, relative: &Path, root: &Path) -> Result<PathBuf, String> {
    let mut normalized = PathBuf::new();
    let absolute = if relative.is_absolute() {
        relative.to_path_buf()
    } else if base.is_absolute() {
        base.join(relative)
    } else {
        root.join(base).join(relative)
    };
    if !absolute.starts_with(root) {
        return Err("module path escapes repository boundary".into());
    }
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("module path escapes repository boundary".into());
                }
                if !normalized.starts_with(root) {
                    return Err("module path escapes repository boundary".into());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if !normalized.starts_with(root) {
        return Err("module path escapes repository boundary".into());
    }
    Ok(normalized)
}

fn canonical_source(root: &Path, path: &Path) -> Result<PathBuf, (&'static str, String)> {
    let canonical = path.canonicalize().map_err(|error| {
        let code =
            if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
                "SYMLINK_RESOLUTION_FAILED"
            } else {
                "SOURCE_INACCESSIBLE"
            };
        (
            code,
            format!("source path cannot be resolved ({})", error.kind()),
        )
    })?;
    if !canonical.starts_with(root) {
        return Err((
            "SOURCE_OUTSIDE_REPOSITORY",
            "source path resolves outside the selected repository boundary".into(),
        ));
    }
    if !canonical.is_file() {
        return Err((
            "SOURCE_NOT_FILE",
            "module source path is not a regular file".into(),
        ));
    }
    Ok(canonical)
}

fn relative_to(root: &Path, path: &Path) -> Result<RepoPath, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "source path resolves outside the selected repository boundary".to_owned())?;
    let value = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    RepoPath::new(if value.is_empty() { "." } else { &value })
        .map_err(|_| "source path is not a normalized repository-relative path".to_owned())
}

fn relative_lexical_to(root: &Path, path: &Path) -> Result<RepoPath, String> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("module path escapes repository boundary".into());
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    let relative = normalized
        .strip_prefix(root)
        .map_err(|_| "module path escapes repository boundary".to_owned())?;
    let value = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    RepoPath::new(if value.is_empty() { "." } else { &value })
        .map_err(|_| "module path is not a normalized repository-relative path".to_owned())
}

fn span_to_source(span: Span, source: &str) -> SourceSpan {
    let start = span.start();
    let end = span.end();
    SourceSpan {
        start_byte: byte_offset(source, start),
        end_byte: byte_offset(source, end),
        start_line: Some(start.line as u32),
        start_column: Some(start.column as u32 + 1),
        end_line: Some(end.line as u32),
        end_column: Some(end.column as u32 + 1),
    }
}

fn byte_offset(source: &str, position: LineColumn) -> u64 {
    let mut offset = 0usize;
    for (line_index, segment) in source.split_inclusive('\n').enumerate() {
        if line_index + 1 == position.line {
            return (offset + position.column).min(source.len()) as u64;
        }
        offset += segment.len();
    }
    source.len() as u64
}
