use std::fmt::Write;
use id_arena::{ArenaBehavior, DefaultArenaBehavior, Id};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use wit_bindgen_core::{
    Files, InterfaceGenerator as _, Ns, Source, WorldGenerator, uwriteln, uwrite, wit_parser,
};
use wit_parser::*;
use anyhow::Result;
use heck::{ToLowerCamelCase, ToShoutySnakeCase, ToUpperCamelCase};
use wit_bindgen_core::abi::AbiVariant;
use wit_bindgen_core::wit_parser::FunctionKind::Freestanding;
// TODO throughout this file, there is a bunch of commented out code from the old prototype. Review and delete/implement these

// TODO maybe use bitflags crate instead
// TODO better kotlin_name
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum OutsideKind {
    Imported,
    Exported,
    Both
}



impl OutsideKind {
    fn also_import(&self) -> OutsideKind {
        match self {
            OutsideKind::Imported => OutsideKind::Imported,
            OutsideKind::Exported => OutsideKind::Both,
            OutsideKind::Both => OutsideKind::Both,
        }
    }
    fn also_export(&self) -> OutsideKind {
        match self {
            OutsideKind::Imported => OutsideKind::Both,
            OutsideKind::Exported => OutsideKind::Exported,
            OutsideKind::Both => OutsideKind::Both,
        }
    }

    fn is_imported(&self) -> bool {
        match self {
            OutsideKind::Exported => false,
            _ => true,
        }
    }

    fn is_exported(&self) -> bool {
        match self {
            OutsideKind::Imported => false,
            _ => true,
        }
    }
}

#[derive(Default)]
struct GenerationPlan {
    interfaces: HashMap<ReferencedInterface, OutsideKind>,
    // the same in-place function cannot be declared imported and exported at the same time, so just tracking them without being able to retrieve them easily is enough
    in_place_funcs: Vec<(String, Function, OutsideKind)>,
}

#[derive(Default)]
struct Kotlin {
    src: Source,
    export_stubs_src: Source,
    opts: Opts,
    names: Ns,
    world_name: String,
    sizes: SizeAlign,

    generation_plan: GenerationPlan,

    world_id: Option<WorldId>, // None means uninitialized
    tuple_counts: HashSet<usize>,
    // TODO maybe redesign to be referencedinterface?
    kotlin_interface_names: HashMap<InterfaceId, String>,
    kotlin_type_aliases: HashMap<TypeId, /*fqn*/ String>,
    // exported_resources: HashSet<TypeId>,
}

/// WIT interfaces themselves don't always have a kotlin_name associated with them, e.g. if they are declared inline in a world.
/// This struct guarantees a kotlin_name instead. Thus, it is a strictly more "powerful" version of both:
/// - WorldKey: because even for interfaces that are identified by their kotlin_name in the world (WorldKey::Name), the id is available here
/// - InterfaceId: because even for interfaces that are identified by their id but anonymous in their definition, their kotlin_name is available here
/// TODO fix this comment it sounds terrible
#[derive(Debug, Clone, Eq)]
struct ReferencedInterface{
    kotlin_name: String,
    /// fully qualified wit interface name, i.e. with package and version
    fq_wit_name: String,
    id: InterfaceId,
}

impl ReferencedInterface{
    fn create_unified_referenced_interface_name(resolve: &Resolve, world_key: &WorldKey, id: InterfaceId) -> ReferencedInterface {
        let kotlin_name = kotlin_interface_name_from_world_key(resolve, &world_key);
        let fq_wit_name = resolve.name_world_key(&world_key);
        let id = unify_interface_id_by_package(resolve, id);
        ReferencedInterface{ kotlin_name, fq_wit_name, id }
    }
}

impl Hash for ReferencedInterface {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl PartialEq<Self> for ReferencedInterface {
    fn eq(&self, other: &Self) -> bool {
        // TODO to make things easier, could even think about unifying ids in here, not on the upper level. But might be a bit annoying
        self.id == other.id
    }
}

struct InterfaceGenerator<'a> {
    src_fragment: Source,
    export_stubs_src_fragment: Source,
    interface_import_export_state: OutsideKind,
    kotlin_gen: &'a mut Kotlin,
    resolve: &'a Resolve,
    // other backends use an Option<(InterfaceId, &'a WorldKey)> here, but that is quite confusing, as it duplicates information
    // TODO so far, don't see the use of this, seems better to just pass stuff directly to functions
    // interface: Option<ReferencedInterface>,
    // TODO ??? (from C backend, only used for resources there)
    // wasm_import_module: Option<&'a str>,
    // TODO
    // private_top_level_src: Source,
}

#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Opts {
    /// Generate stubs for export implementation
    #[cfg_attr(feature = "clap", arg(long, default_value = "true"))]
    pub generate_stubs: bool, // TODO actually use

    /// Name for the interface containing the in-place/world-scope functions
    #[cfg_attr(feature = "clap", arg(long, default_value = "InPlaceWorldFunctions"))]
    pub in_place_interface_name: String,
}

impl Opts {
    pub fn build(&self) -> Box<dyn WorldGenerator> {
        let mut r = Kotlin::default();
        r.opts = self.clone();
        Box::new(r)
    }
}

// TODO do we need something akin to remove_types_redefined_by_exports of the C backend?

impl WorldGenerator for Kotlin {
    fn preprocess(&mut self, resolve: &Resolve, world: WorldId) {
        let name = &resolve.worlds[world].name;
        self.world_name = name.to_string();
        self.sizes.fill(resolve);
        self.world_id = Some(world);
    }

    fn import_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let referenced_interface = ReferencedInterface::create_unified_referenced_interface_name(resolve, name, id);

        self.kotlin_interface_names.insert(referenced_interface.id, referenced_interface.kotlin_name.clone());

        // if it doesn't exist: create it as an import; if it exists: also import it
        self.generation_plan.interfaces.entry(referenced_interface)
            .and_modify(|kind| *kind = kind.also_import())
            .or_insert(OutsideKind::Imported);

        Ok(())
    }

    fn export_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let referenced_interface = ReferencedInterface::create_unified_referenced_interface_name(resolve, name, id);

        self.kotlin_interface_names.insert(referenced_interface.id, referenced_interface.kotlin_name.clone());

        self.generation_plan.interfaces.entry(referenced_interface)
            .and_modify(|kind| *kind = kind.also_export())
            .or_insert(OutsideKind::Exported);

        Ok(())
    }

    fn import_funcs(
        &mut self,
        _resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) {
        debug_assert_eq!(self.world_id, Some(world));

        for (name, func) in funcs {
            self.generation_plan.in_place_funcs.push((name.to_string(), (*func).clone(), OutsideKind::Imported));
        }
    }

    fn export_funcs(
        &mut self,
        _resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) -> Result<()> {
        debug_assert_eq!(self.world_id, Some(world));

        for (name, func) in funcs {
            self.generation_plan.in_place_funcs.push((name.to_string(), (*func).clone(), OutsideKind::Exported));
        }
        Ok(())
    }

    fn import_types(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        types: &[(&str, TypeId)],
        _files: &mut Files,
    ) {
        // TODO actually implement this correctly, all of this is just experiments
        debug_assert_eq!(self.world_id, Some(world));

        for (name, id) in types {
            // self.generation_plan.types.add_type_id(resolve, *id);

            // TODO question: what should the kotlin_name be, then? Probably the `use` statements just shouldn't introduce new names
            let resolved_ty = resolve_use_type_fully(resolve, id);
            unsafe { debug_assert!(name.to_string().eq(resolved_ty.name.as_ref().unwrap_unchecked())); }

            println!("{}, {:?}", name, resolved_ty)
        }
    }

    fn finish(&mut self, resolve: &Resolve, id: WorldId, files: &mut Files) -> Result<()> {
        debug_assert_eq!(self.world_id, Some(id));

        let Some(package_id) = resolve.worlds[id].package else { todo!("What kind of black magic is this") };
        let package = &resolve.packages[package_id];
        // TODO use version info
        let kotlin_package_name = format!("{}.{}", package.name.namespace, package.name.name.to_lower_camel_case());
        uwriteln!(self.src, "/*package TODO {}*/", kotlin_package_name);
        uwriteln!(self.src, "/*@file:WitPackage(TODO {})*/", resolve.packages[package_id].name);

        // move generation plan out, so that we don't borrow from self twice
        let generation_plan = std::mem::take(&mut self.generation_plan);

        for (referenced_interface, outside_kind) in generation_plan.interfaces {
            println!("{:?}: {:?}", referenced_interface, outside_kind);

            // let iface_id = referenced_interface.id;
            // let iface_name = referenced_interface.kotlin_name.clone();

            let (src_fragment, export_stubs_src_fragment) = {
                let mut generator = self.generator(resolve, outside_kind);
                // TODO see generator.interface definition
                // generator.interface = Some(referenced_interface);

                for(name, ty) in &resolve.interfaces[referenced_interface.id].types {
                    generator.define_type(name, *ty);
                }

                if outside_kind.is_exported() {
                    uwriteln!(generator.export_stubs_src_fragment, "@WitExport/*(TODO)*/\nobject {}Impl : {}{{", referenced_interface.kotlin_name, referenced_interface.kotlin_name);
                }

                let num_funcs = resolve.interfaces[referenced_interface.id].functions.len();
                for (i, (_, func)) in resolve.interfaces[referenced_interface.id].functions.iter().enumerate() {
                    // TODO deal with non freestanding/understand them. seems its only resource and async-related
                    debug_assert_eq!(func.kind, Freestanding);
                    generator.define_function(func, outside_kind/*, &*referenced_interface.kotlin_name*/);
                    if i < num_funcs - 1 {
                    generator.append_just_ln();
                    }
                }

                if outside_kind.is_exported() {
                    uwriteln!(generator.export_stubs_src_fragment, "}}");
                }
                
                (generator.src_fragment, generator.export_stubs_src_fragment)
            };

            uwriteln!(self.src, "@WitInterface(/*TODO*/\"{}\")\nexternal interface {} {{", referenced_interface.fq_wit_name, referenced_interface.kotlin_name);
            if outside_kind.is_imported() {
                // TODO could maybe leave this out if the interface doesn't define any functions
                uwriteln!(self.src, "@WitImport/*(TODO)*/\ncompanion object Import : {}/* by stdlib.witMagicIntrinsic()*/", referenced_interface.kotlin_name);
            }
            self.src.push_str(src_fragment.as_str());
            uwriteln!(self.src, "}}");

            self.export_stubs_src.push_str(export_stubs_src_fragment.as_str());
            self.export_stubs_src.push_str("\n");
        }

        if !generation_plan.in_place_funcs.is_empty() {
            uwriteln!(self.src, "@WitInterface(TODO)\nexternal interface {} {{", self.opts.in_place_interface_name);
            for (fn_name, func, outside_kind) in &generation_plan.in_place_funcs {
                let (src_fragment, export_stubs_src_fragment) = {
                    let mut generator = self.generator(resolve, *outside_kind);
                    // TODO maybe get rid of generator.interface entirely if possible?
                    generator.define_function(func, *outside_kind);

                    (generator.src_fragment, generator.export_stubs_src_fragment)
                };
                self.src.push_str(src_fragment.as_str());
                // TODO newlines and prettyness
            }
            uwriteln!(self.src, "}}");
        }


        println!("src:\n{}", self.src.as_str());
        println!("export stubs src:\n{}", self.export_stubs_src.as_str());

        files.push(&format!("{}.kt", self.world_name), self.src.as_ref());
        files.push(&format!("{}ExportStubs.kt", self.world_name), self.export_stubs_src.as_ref());

        Ok(())
    }
}

/// resolve.interfaces contains every interface once per import/export.
/// However, we want one unified interface id for both.
/// This can be achieved by accessing the package of the interface, and getting the interface from its list of interfaces, which is deduplicated by kotlin_name, and thus has each interface exactly once, even if it is imported and exported
fn unify_interface_id_by_package(resolve: &Resolve, original_id: InterfaceId) -> InterfaceId {
    let package = resolve.interfaces[original_id].package;
    if let None = package {
        // TODO think about the implications of this. It should be correct, because there's no way to reference this same iface twice (for import and export) anyway
        return original_id;
    }
    let package = package.unwrap();

    let original_iface_name = &resolve.interfaces[original_id].name;
    if let None = original_iface_name {
        // TODO probably same as above
        return original_id;
    }
    let original_iface_name = original_iface_name.as_ref().unwrap();

    resolve.packages[package].interfaces[original_iface_name]
}

impl Kotlin {
    fn generator<'a>(
        &'a mut self,
        resolve: &'a Resolve,
        interface_import_export_state: OutsideKind,
    ) -> InterfaceGenerator<'a> {
        InterfaceGenerator {
            src_fragment: Source::default(),
            export_stubs_src_fragment: Source::default(),
            interface_import_export_state,
            kotlin_gen: self,
            resolve,
        }
    }
}


impl<'a> wit_bindgen_core::InterfaceGenerator<'a> for InterfaceGenerator<'a> {
    fn resolve(&self) -> &'a Resolve {
        self.resolve
    }

    fn type_record(&mut self, _id: TypeId, name: &str, record: &Record, docs: &Docs) {
        self.append_just_ln();
        self.append(kdoc(docs).as_str());
        self.append("data class ");
        let name = name.to_upper_camel_case();
        self.append(&name);
        self.append_ln("(");
        // TODO(Kotlin): ident doesn't work
        for field in record.fields.iter() {
            self.append(kdoc(&field.docs).as_str());
            self.append("var ");
            self.append(&to_kotlin_identifier(&field.name));
            self.append(": ");
            let ty = &field.ty;
            self.append(self.type_name(ty).as_str());
            self.append_ln(",");
        }
        self.append_ln(")")
    }

    fn type_resource(&mut self, type_id: TypeId, name: &str, docs: &Docs) {
        todo!();

        /*
        if !self.in_import {
            self.gen.exported_resources.insert(type_id);
        }

        let camel = kotlin_name.to_upper_camel_case();

        let import_module : String = match self.interface {
            Some((_, key)) => self.resolve.name_world_key(key),
            None => unimplemented!("resource imports from worlds"),
        };

        let import_module = if self.in_import {
            import_module
        } else {
            format!("[export]{import_module}")
        };

        let imported_function_prefix = self.resource_import_prefix(&type_id);

        self.private_top_level_src.push_str(&format!(
            r#"
                @WasmImport("{import_module}", "[resource-drop]{kotlin_name}")
                internal external fun {imported_function_prefix}_drop(handle: Int): Unit
            "#
        ));

        if !self.in_import {
            self.private_top_level_src.push_str(&format!(
                r#"
                    @WasmImport("{import_module}", "[resource-new]{kotlin_name}")
                    internal external fun {imported_function_prefix}_new(handle: Int): Int

                    @WasmImport("{import_module}", "[resource-rep]{kotlin_name}")
                    internal external fun {imported_function_prefix}_rep(handle: Int): Int
                "#
            ));
        }

        self.append(kdoc(docs).as_str());
        if !self.in_import {
            uwrite!(self.src_fragment, "abstract ")
        }

        uwriteln!(self.src_fragment, "class {camel} : AutoCloseable {{");
        uwriteln!(self.src_fragment, "internal var __handle: ResourceHandle = ResourceHandle(0)");

        if self.in_import { // Exported constructor handle
            uwriteln!(self.src_fragment, "internal constructor(handle: ResourceHandle) {{ __handle = handle }}");
        }

        // TODO: Zero out the handle
        uwriteln!(self.src_fragment, "override fun close() {{ {imported_function_prefix}_drop(__handle.value) }} ");

        let ty = &self.resolve.types[type_id];
        let mut functions: Vec<&Function> = Vec::new();
        match ty.owner {
            TypeOwner::Interface(id) => {
                let interface = &self.resolve.interfaces[id];
                for (_, f) in &interface.functions {
                    functions.push(f);
                }
            }
            TypeOwner::World(id) => {
                let world = &self.resolve.worlds[id];
                for (_, import) in world.imports.iter() {
                    match import {
                        WorldItem::Function(f) => functions.push(f),
                        _ => {}
                    }
                }
            }
            TypeOwner::None => unimplemented!("Resource without type owner")
        }

        if !self.in_import {
            self.export_stubs_src.push_str(kdoc(docs).as_str());

            let namespace_name = self.namespace_name.clone().unwrap();

            let mut has_constructor: bool = false;
            for f in &functions {
                match f.kind {
                    FunctionKind::Constructor(id) if id == type_id => { has_constructor = true; }
                    _ => {}
                }
            }
            // If exported resource doesn't have a constructor, call the primary super constructor
            let maybe_super_constructor_call = if has_constructor { "" } else { "()" };

            uwriteln!(self.export_stubs_src, "class {camel}Impl : {namespace_name}.{camel}{maybe_super_constructor_call} {{");
        }


        let interface_name = self.interface.map(|(_, k)| k);

        for f in &functions {
            match f.kind {
                FunctionKind::Method(id) | FunctionKind::Constructor(id) if id == type_id => {
                    if self.in_import {
                        self.import(f, interface_name);
                    } else {
                        self.export(f, interface_name);
                    }
                }
                _ => {}
            }
        }

        if !self.in_import {
            uwriteln!(self.src_fragment, "interface Statics {{");
            uwriteln!(self.export_stubs_src, "companion object : Statics {{");
        } else {
            uwriteln!(self.src_fragment, "companion object {{");
        }

        for f in &functions {
            match f.kind {
                FunctionKind::Static(id) if id == type_id => {
                    if self.in_import {
                        self.import(f, interface_name);
                    } else {
                        self.export(f, interface_name);
                    }
                }
                _ => {}
            }
        }
        self.append("}");
        self.append("}");

        if !self.in_import {
            self.export_stubs_src.push_str("}");
            self.export_stubs_src.push_str("}");
        }
        */
    }

    fn type_flags(&mut self, _id: TypeId, name: &str, flags: &Flags, docs: &Docs) {
        self.append("\n");
        self.append(kdoc(docs).as_str());
        self.append("value class ");
        let name = name.to_upper_camel_case();
        self.append(&name);
        // TODO(Kotlin): Support underlying values smaller than Long
        self.append(" internal constructor(val _value: Long) {\n");
        self.append("constructor(\n");
        for flag in flags.flags.iter() {
            uwrite!(
                self.src_fragment,
                "{}: Boolean = false,",
                flag.name.to_lower_camel_case(),
            );
        }
        self.append("\n) : this(0L");
        for (i, flag) in flags.flags.iter().enumerate() {
            uwrite!(
                self.src_fragment,
                " or (if ({}) (1L shl {i}) else 0L)",
                flag.name.to_lower_camel_case(),
            );
        }
        self.append(")\n");
        for (i, flag) in flags.flags.iter().enumerate() {
            uwriteln!(
                self.src_fragment,
                "val {}: Boolean get() = (_value and (1L shl {i})) != 0L",
                flag.name.to_lower_camel_case(),
            );
        }
        // TODO(Kotlin): Add toString method

        self.append("}\n");
    }

    fn type_tuple(&mut self, _id: TypeId, _name: &str, _tuple: &Tuple, _docs: &Docs) {}

    fn type_variant(&mut self, _id: TypeId, name: &str, variant: &Variant, docs: &Docs) {
        self.append("\n");
        self.append(kdoc(docs).as_str());
        self.append("sealed interface ");
        let variant_name = name.to_upper_camel_case();
        self.append(&variant_name);
        self.append("{ \n");
        for case in &variant.cases {
            let case_name = case.name.to_upper_camel_case();
            match &case.ty {
                None => {
                    self.append("data object ");
                    self.append(case_name.as_str());
                }
                Some(ty) => {
                    self.append("data class ");
                    self.append(case_name.as_str());
                    self.append("(val value: ");
                    // TODO this does an unnecessary copy. The alternative would be to append to src_fragment, but that would require a mutable reference to self.src_fragment, which conflicts with the immutable borrow by calling the function...
                    self.append(self.type_name(ty).as_str());
                    self.append(")");
                }
            }
            self.append(" : ");
            self.append(&variant_name);
            self.append("\n");
        }
        self.append("}");
    }

    // TODO why are these empty?
    fn type_option(&mut self, _id: TypeId, _name: &str, _payload: &Type, _docs: &Docs) {}
    fn type_result(&mut self, _id: TypeId, _name: &str, _result: &Result_, _docs: &Docs) {}
    fn type_enum(&mut self, _id: TypeId, name: &str, enum_: &Enum, docs: &Docs) {
        self.append_just_ln();
        self.append(kdoc(docs).as_str());
        self.append("enum class ");
        let name = name.to_upper_camel_case();
        self.append(&name);
        self.append(" {\n");
        for case in enum_.cases.iter() {
            self.append(kdoc(&case.docs).as_str());
            self.append(case.name.to_shouty_snake_case().as_str());
            self.append(",\n");
        }
        self.append("}\n");
    }
    // Kotlin does now support nested type aliases, issue: https://youtrack.jetbrains.com/issue/KT-45285
    fn type_alias(&mut self, id: TypeId, name: &str, ty: &Type, docs: &Docs) {
        // TODO with wit `use` these aliases get defined where they're used, not where they're defined...
        //      so:
        //      interface types{
        //          type T = tuple<u32,u16>;
        //      }
        //
        //      ... in some interface 'iface' ... use types.{T};
        //
        //      results in the type being defined in 'iface'...
        //      but judging from the json of the resolve struct, this should be possible to fix

        // TODO this re-defines already named types in a somewhat awkward way when they're used.
        //      Should do some check here to see if its a type that is named (e.g. record, flags, etc.), and if it is, don't do anything here.

        self.append_just_ln();
        self.append(kdoc(docs).as_str());
        self.append("typealias ");
        let name = name.to_upper_camel_case();
        self.append(&name);
        self.append(" = ");
        self.append_ln(self.type_name(ty).as_str());

        let namespace_name = match self.resolve.types[id].owner {
            TypeOwner::World(_) => self.kotlin_gen.opts.in_place_interface_name.as_str(),
            TypeOwner::Interface(iface_id) => self.kotlin_gen.kotlin_interface_names[&unify_interface_id_by_package(self.resolve, iface_id)].as_str(),
            TypeOwner::None => todo!("type alias without owner")
        };

        let fqn = format!("{}.{}", namespace_name, name);

        self.kotlin_gen.kotlin_type_aliases.insert(id, fqn);
    }
    fn type_list(&mut self, _id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {}
    fn type_builtin(&mut self, _id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {}

    fn type_future(&mut self, id: TypeId, name: &str, ty: &Option<Type>, docs: &Docs) {
        todo!()
    }

    fn type_stream(&mut self, id: TypeId, name: &str, ty: &Option<Type>, docs: &Docs) {
        todo!()
    }
}

impl InterfaceGenerator<'_> {
    // fn interface_name(&self) -> String {
    //     kotlin_interface_name(self.resolve, &self.interface.unwrap().id)
    // }

    fn append(&mut self, src: &str) {
        uwrite!(self.src_fragment, "{}", src);
    }

    fn append_ln(&mut self, src: &str) {
        uwriteln!(self.src_fragment, "{}", src);
    }

    fn append_just_ln(&mut self) {
        self.append("\n");
    }

    fn resource_import_prefix(&self, id: &TypeId) -> String {
        todo!()
        /*
        let mut result = String::new();
        let is_exported = self.kotlin_gen.exported_resources.contains(&id);
        let ty = &self.resolve.types[*id];

        match &ty.owner {
            TypeOwner::Interface(ty_interface_id) => {
                let namespace_name = &self.kotlin_gen.kotlin_interface_names[ty_interface_id];
                if is_exported {
                    uwrite!(result, "{namespace_name}Impl");
                } else {
                    uwrite!(result, "{namespace_name}");
                }
            }
            TypeOwner::World(_) => {
                // TODO(KT): World fqn
            }
            TypeOwner::None => {}
        }

        match &ty.kotlin_name {
            None => {}
            Some(kotlin_name) => {
                let kotlin_name = kotlin_name.to_upper_camel_case();
                uwrite!(result, "_{kotlin_name}");
            }
        }

        let common_prefix = "__cm_resource_abi";
        return if is_exported {
            format!("{common_prefix}_export_{result}")
        } else {
            format!("{common_prefix}_import_{result}")
        };
        */
    }

    fn type_name(&self, ty: &Type) -> String {
        let mut name = String::new();
        self.push_type_name(ty, &mut name);
        name
    }


    fn push_type_name(&self, outerTy: &Type, dst: &mut String) {
        match outerTy {
            Type::Bool => dst.push_str("Boolean"),
            Type::Char => dst.push_str("Int"), // TODO: Find a better type?
            Type::U8 => dst.push_str("UByte"),
            Type::S8 => dst.push_str("Byte"),
            Type::U16 => dst.push_str("UShort"),
            Type::S16 => dst.push_str("Short"),
            Type::U32 => dst.push_str("UInt"),
            Type::S32 => dst.push_str("Int"),
            Type::U64 => dst.push_str("ULong"),
            Type::S64 => dst.push_str("Long"),
            Type::F32 => dst.push_str("Float"),
            Type::F64 => dst.push_str("Double"),
            Type::String => dst.push_str("String"),
            Type::Id(id) => {
                let ty = &self.resolve.types[*id];
                match &ty.kind {
                    // alias
                    TypeDefKind::Type(t) => dst.push_str(self.kotlin_gen.kotlin_type_aliases[&id].as_str()),
                    TypeDefKind::Record(_)
                    | TypeDefKind::Resource
                    | TypeDefKind::Flags(_)
                    | TypeDefKind::Enum(_)
                    | TypeDefKind::Variant(_) => {
                        // TODO exported resources thing
                        // let is_exported_resource = self.kotlin_gen.exported_resources.contains(id);
                        match &ty.owner {
                            TypeOwner::Interface(ty_interface_id) => {
                                let namespace_name = &self.kotlin_gen.kotlin_interface_names[&unify_interface_id_by_package(self.resolve, *ty_interface_id)];
                                // if is_exported_resource {
                                //     uwrite!(dst, "{namespace_name}Impl.");  // Exported resources live only in Implementation namespace
                                // } else {
                                    uwrite!(dst, "{namespace_name}.");
                                // }
                            }
                            TypeOwner::World(_) => {
                                // TODO(KT): World fqn
                            }
                            TypeOwner::None => {}
                        }

                        if let Some(name) = &ty.name {
                            dst.push_str(&name.to_upper_camel_case());
                            // if is_exported_resource {
                            //     dst.push_str("Impl")
                            // }
                        } else {
                            unreachable!();
                        }
                    }
                    TypeDefKind::Tuple(tuple) => {
                        match tuple.types.len() {
                            0 => {
                                dst.push_str("Unit");
                            }
                            1 => {
                                self.push_type_name(&tuple.types[0], dst);
                            }
                            2 => {
                                dst.push_str("Pair<");
                                self.push_type_name(&tuple.types[0], dst);
                                dst.push_str(", ");
                                self.push_type_name(&tuple.types[1], dst);
                                dst.push_str(">");
                            }
                            3 => {
                                dst.push_str("Triple<");
                                self.push_type_name(&tuple.types[0], dst);
                                dst.push_str(", ");
                                self.push_type_name(&tuple.types[1], dst);
                                dst.push_str(", ");
                                self.push_type_name(&tuple.types[2], dst);
                                dst.push_str(">");
                            }
                            len => {
                                uwrite!(dst, "Tuple{len}<");
                                for (idx, ty) in tuple.types.iter().enumerate() {
                                    if idx != 0 {
                                        uwrite!(dst, ", ");
                                    }
                                    self.push_type_name(ty, dst);
                                }
                                dst.push_str(">");
                            }
                        }
                    }
                    TypeDefKind::Option(ty) => {
                        let has_nested_option_type = match ty {
                            Type::Id(id) => match self.resolve.types[*id].kind {
                                TypeDefKind::Option(_) => true,
                                _ => false
                            }
                            _ => false
                        };

                        if has_nested_option_type {
                            dst.push_str("Option<");
                            self.push_type_name(ty, dst);
                            dst.push_str(">");
                        } else {
                            // Non-nested options are Kotlin nullable "?" types.
                            self.push_type_name(ty, dst);
                            dst.push_str("?");
                        }
                    }
                    TypeDefKind::Result(r) => {
                        dst.push_str("Result<");
                        match &r.ok {
                            Some(ty) => self.push_type_name(ty, dst),
                            None => dst.push_str("Unit"),
                        }
                        dst.push_str(">");
                    }
                    TypeDefKind::List(ty) => {
                        dst.push_str("List<");
                        self.push_type_name(ty, dst);
                        dst.push_str(">");
                    }
                    TypeDefKind::Future(_) => unimplemented!(),
                    TypeDefKind::Stream(_) => unimplemented!(),
                    TypeDefKind::Handle(Handle::Own(resource)) => {
                        self.push_type_name(&Type::Id(*resource), dst);
                    }
                    TypeDefKind::Handle(Handle::Borrow(resource)) => {
                        self.push_type_name(&Type::Id(*resource), dst);
                    }
                    TypeDefKind::Unknown => unreachable!(),
                    TypeDefKind::Map(_, _) => {}
                    TypeDefKind::FixedLengthList(_, _) => {}
                }
            }
            Type::ErrorContext => {todo!()}
        }
    }

    /*
    fn import(&mut self, func: &Function, interface_name: Option<&WorldKey>) {
        let sig = self.resolve.wasm_signature(AbiVariant::GuestImport, func);
        self.private_top_level_src.push_str("\n");

        uwriteln!(
            self.private_top_level_src,
            "@WasmImport(\"{}\", \"{}\")",
            match interface_name {
                Some(kotlin_name) => self.resolve.name_world_key(kotlin_name),
                None => "\\$root".to_string(),  // TODO(Kotlin): Escape all strings properly
            },
            func.kotlin_name
        );
        let kotlin_name = kotlin_fun_name(func);
        let import_name = self.gen.names.tmp(&format!("__wasm_import_{kotlin_name}",));
        self.private_top_level_src.push_str("internal external fun ");
        self.private_top_level_src.push_str(&import_name);
        self.private_top_level_src.push_str("(");
        for (i, param) in sig.params.iter().enumerate() {
            if i > 0 {
                self.private_top_level_src.push_str(", ");
            }
            uwrite!(self.private_top_level_src, "p{i}: ");
            self.private_top_level_src.push_str(wasm_type(*param));
        }
        self.private_top_level_src.push_str("): ");
        match sig.results.len() {
            0 => self.private_top_level_src.push_str("Unit"),
            1 => self.private_top_level_src.push_str(wasm_type(sig.results[0])),
            _ => unimplemented!("multi-value return not supported"),
        }
        self.private_top_level_src.push_str("\n");

        self.append(kdoc(&func.docs).as_str());
        self.append("public ");
        {
            let sig = self.kotlin_signature(func);
            self.append(sig.as_str());
            self.append("\n");

        }
        if let FunctionKind::Constructor(_) = func.kind {
            // IIFE in primary construct call
            self.append(": this(ResourceHandle(run(fun (): Int");
        }
        self.append(" {\n");
        self.append("// <editor-fold defaultstate=\"collapsed\" desc=\"Generated Bindings Code\">\n");

        self.append(" withScopedMemoryAllocator { allocator -> \n");

        let mut f = FunctionBindgen::new(self, &import_name, func.kind.clone());
        for (idx, (kotlin_name, _)) in func.params.iter().enumerate() {
            let param = if idx == 0 && matches!(func.kind, FunctionKind::Method(_)) {
                "this".to_string()
            } else {
                to_kotlin_identifier(kotlin_name)
            };
            f.locals.insert(&param).unwrap();
            f.params.push(param.clone());
        }

        abi::call(
            f.gen.resolve,
            AbiVariant::GuestImport,
            LiftLower::LowerArgsLiftResults,
            func,
            &mut f,
        );

        let FunctionBindgen {
            src,
            ..
        } = f;

        self.append(&String::from(src));
        self.append("}\n");
        self.append("// </editor-fold>\n");
        self.append("}\n");
        if let FunctionKind::Constructor(_) = func.kind {
            // End of IIFE in primary construct call
            self.append(")))\n");
        }
    }
     */

    /*
    fn export(&mut self, func: &Function, interface_name: Option<&WorldKey>) {
        let wasm_sig = self.resolve.wasm_signature(AbiVariant::GuestExport, func);

        let core_module_name = interface_name.map(|s| self.resolve.name_world_key(s));
        let export_name = func.core_export_name(core_module_name.as_deref());
        {
            let kotlin_sig = self.kotlin_signature(func);
            if !matches!(func.kind, FunctionKind::Constructor(_)) {  // Constructor in exported abstract resource class is not needed
                uwriteln!(self.src_fragment, "abstract {kotlin_sig}");
                uwriteln!(self.export_stubs_src, "override {kotlin_sig} {{ TODO() }}");
            } else {
                uwriteln!(self.export_stubs_src, "{kotlin_sig} : super() {{ TODO() }}");
            }
        }

        uwriteln!(
            self.private_top_level_src,
            "\n@WasmExport(\"{export_name}\")"
        );
        let kotlin_name = kotlin_fun_name(func);
        let export_fun_name = self.gen.names.tmp(&format!("__wasm_export_{kotlin_name}"));

        let mut f = FunctionBindgen::new(self, &export_fun_name, func.kind.clone());
        let s: &mut Source = &mut f.gen.private_top_level_src;
        s.push_str("fun ");
        s.push_str(&export_fun_name);
        s.push_str("(");
        for (i, param) in wasm_sig.params.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let kotlin_name = format!("p{i}");

            uwrite!(s, "{kotlin_name}: ");
            s.push_str(wasm_type(*param));
            f.params.push(kotlin_name);
        }
        s.push_str("): ");
        match wasm_sig.results.len() {
            0 => s.push_str("Unit"),
            1 => s.push_str(wasm_type(wasm_sig.results[0])),
            _ => unimplemented!("multi-value return not supported"),
        }
        s.push_str(" {\n");
        s.push_str("freeAllComponentModelReallocAllocatedMemory()\n");
        s.push_str(" withScopedMemoryAllocator { allocator -> \n");


        // Perform all lifting/lowering and append it to our src_fragment.
        abi::call(
            f.gen.resolve,
            AbiVariant::GuestExport,
            LiftLower::LiftArgsLowerResults,
            func,
            &mut f,
        );
        let FunctionBindgen { src, .. } = f;
        self.private_top_level_src.push_str(&src);
        self.private_top_level_src.push_str("}\n");
        self.private_top_level_src.push_str("}\n");
    }
     */

    // TODO the whole name handling is a bit cursed, because some things dont have an interface name...
    //       could also try to use self.interface.kotlin_name, but that doesn't exist when
    fn define_function(&mut self, func: &Function, outside_kind: OutsideKind/*, kotlin_interface_name: &str*/) {
        // TODO write to export stubs segment
        let sig = self.resolve.wasm_signature(AbiVariant::GuestImport, func);

        self.append(kdoc(&func.docs).as_str());
        // TODO prob remove, prob dont want this
        // if outside_kind.is_imported() {
        //     self.append_ln("@WitImport(TODO(decide if we want separate @WitImport annotations here. Imo makes more sense to leave them out, because there are also no @WitExport annotations here, which wouldn't make sense either))");
        // }

        // TODO actually don't want external. public maybe
        // self.append("public external ");
        self.append_ln(self.kotlin_signature(func).as_str());

        if outside_kind.is_exported() {
            // TODO probably no function level annotations, right?
            // self.export_stubs_src_fragment.push_str("@WitExport(TODO)\n");
            self.export_stubs_src_fragment.push_str("override ");
            self.export_stubs_src_fragment.push_str(self.kotlin_signature(func).as_str());
            self.export_stubs_src_fragment.push_str("{\n");
            self.export_stubs_src_fragment.push_str("TODO(\"wit-bindgen autogenerated stub\")\n");
            self.export_stubs_src_fragment.push_str("}\n");
        }

        // TODO this is wrong, I think. The wit docs say somewhere, that resource constructors are just syntactic sugar, but we're not treating them as such here
        // if let FunctionKind::Constructor(_) = func.kind {
        //     // TODO fix with new resources. Do we even need this here, or is it entirely in the compiler in the new prototype?
        //     // IIFE in primary construct call
        //     self.append(": this(ResourceHandle(run(fun (): Int");
        // }
    }

    fn kotlin_signature(&self, func: &Function) -> String {
        let mut result = String::new();

        let name = kotlin_fun_name(func);
        if let FunctionKind::Constructor(_) = func.kind {
            result.push_str("constructor");
        } else {
            result.push_str("fun ");
            result.push_str(&name);
        }
        result.push_str("(");
        for (i, param) in func.params.iter().enumerate() {
            let (name, ty) = (&*param.name, &param.ty);
            if let FunctionKind::Method(_) = func.kind {
                if i == 0 { continue }
                if i > 1 { result.push_str(", "); }
            } else {
                if i > 0 { result.push_str(", "); }
            }
            result.push_str(&to_kotlin_identifier(name));
            result.push_str(": ");
            self.push_type_name(ty, &mut result)
        }
        result.push_str(")");
        if let FunctionKind::Constructor(_) = func.kind {
            return result;
        }

        match &func.result {
            /*
            Results::Named(params) => {
                match params.len() {
                    0 => result.push_str("Unit"),
                    1 => result.push_str(self.type_name(&params[0].1).as_str()),
                    count => {
                        self.gen.tuple_counts.insert(count);
                        uwrite!(
                            result,
                            "Tuple{count}<{}>",
                            func.results
                                .iter_types()
                                .map(|ty| self.type_name(ty))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
            }
            Results::Anon(ty) => {
                result.push_str(self.type_name(ty).as_str());
            }
             */
            None => {}
            Some(ty) => {
                result.push_str(": ");
                self.push_type_name(ty, &mut result);
            }
        }
        result
    }
}


//TODO see if this makes sense
fn resolve_use_type_fully<'a>(resolve: &'a Resolve, id: &TypeId) -> &'a TypeDef {
    let ty = &resolve.types[*id];
    match ty.kind {
        // TODO it looks like if its defined by Type and Type::Id, its not a type alias, its a `use` declaration using the same type somewhere else, but if its anything else under Type, its an actual type alias. So be explicit about both cases
        TypeDefKind::Type(Type::Id(id)) => resolve_use_type_fully(resolve, &id),
        TypeDefKind::Type(_) => ty,
        _ => ty
    }
}

fn kdoc(docs: &Docs) -> String {
    if let Some(docs) = &docs.contents {
        // https://kotlinlang.org/docs/kotlin-doc.html#kdoc-syntax
        format!("/**\n{docs}\n*/\n")
    } else {
        String::new()
    }
}

pub fn to_kotlin_identifier(name: &str) -> String {
    match name {
        // Escape Kotlin keywords
        // Source: https://kotlinlang.org/docs/keyword-reference.html#hard-keywords
        "as" |
        "break" | "class" | "continue" | "do" | "else" | "false" |
        "for" | "fun" | "if" | "in" | "interface" | "is" | "null" |
        "object" | "package" | "return" | "super" | "this" | "throw" |
        "true" | "try" | "typealias" | "typeof" | "val" | "var" |
        "when" | "while"
        => name.to_owned() + "_",
        // ret and err needs to be escaped because they are used as
        //  variable names for option and result flattening.
        "ret" => "ret_".into(),
        "err" => "err_".into(),
        s => s.to_lower_camel_case(),
    }
}
fn kotlin_fun_name(func: &Function) -> String {
    to_kotlin_identifier(func.item_name())
}

/// This differs from resolve.name_world_key(key) in that that one returns the wit kotlin_name of the interface, whereas we need the kotlin kotlin_name
fn kotlin_interface_name_from_world_key(resolve: &Resolve, key: &WorldKey) -> String {
    match key {
        WorldKey::Name(n) => n.to_string(),
        WorldKey::Interface(inner_id) => {
            // TODO is there any case where this unwrap can fail?
            resolve.interfaces[*inner_id].name.clone().unwrap().to_upper_camel_case()
        }
    }
}

fn fully_qualified_wit_interface_name_from_world_key(resolve: &Resolve, key: &WorldKey) -> String {
    resolve.name_world_key(key)
}

// fn kotlin_interface_name(re world_key: &Wsolve: &Resolve,orldKey) -> String {
//     // does not depend on import/export, as we generate only one kotlin interface for both
//     match world_key {
//         WorldKey::Name(kotlin_name) => kotlin_name.to_upper_camel_case(),
//         WorldKey::Interface(id) => match &resolve.interfaces[*id].kotlin_name {
//             None => "AnonymousInterface".to_string(),
//             Some(kotlin_name) => kotlin_name.to_upper_camel_case()
//         },
//     }
// }
//
