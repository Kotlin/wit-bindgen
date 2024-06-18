use anyhow::Result;
use heck::*;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::mem;
use wit_bindgen_core::abi::{self, AbiVariant, Bindgen, Bitcast, Instruction, LiftLower, WasmType};
use wit_bindgen_core::{uwrite, uwriteln, wit_parser::*, Direction, Files, InterfaceGenerator as _, Ns, WorldGenerator, Source, dealias};

#[derive(Default)]
struct Kotlin {
    src: Source,
    private_src: Source,
    export_stubs_src: Source,
    opts: Opts,
    names: Ns,
    world: String,
    sizes: SizeAlign,

    world_id: Option<WorldId>,
    tuple_counts: HashSet<usize>,
    interface_names: HashMap<InterfaceId, String>,
    exported_resources: HashSet<TypeId>,
}

#[derive(Default)]
pub struct ResourceInfo {
    pub direction: Direction,
}

#[derive(Default, Debug, Clone)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Opts {
    /// Generate subs export implementation
    #[cfg_attr(feature = "clap", arg(long))]
    pub generate_stubs: bool,
}

impl Opts {
    pub fn build(&self) -> Box<dyn WorldGenerator> {
        let mut r = Kotlin::default();
        r.opts = self.clone();
        Box::new(r)
    }
}

impl WorldGenerator for Kotlin {
    fn preprocess(&mut self, resolve: &Resolve, world: WorldId) {
        let name = &resolve.worlds[world].name;
        self.world = name.to_string();
        self.sizes.fill(resolve);
        self.world_id = Some(world);
    }

    fn import_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) {
        let namespace_name = interface_namespace_name(&resolve, &id, true);
        self.interface_names.insert(id, namespace_name.clone());

        let mut gen = self.interface(resolve, true, Some(namespace_name.to_string()));
        gen.interface = Some((id, name));

        for (name, ty) in &resolve.interfaces[id].types {
            gen.define_type(name, *ty);
        }

        for (_, func) in &resolve.interfaces[id].functions {
            if func.kind == FunctionKind::Freestanding {
                gen.import(func, Some(name));
            }
        }

        let object_body =  &gen.src.as_mut_string();
        let private_top_level_body = &gen.private_top_level_src.as_mut_string();
        uwriteln!(self.src, "object {namespace_name} {{\n{object_body}\n}}\n");
        uwriteln!(self.private_src, "{private_top_level_body}\n");
    }

    fn export_interface(
        &mut self,
        resolve: &Resolve,
        name: &WorldKey,
        id: InterfaceId,
        _files: &mut Files,
    ) -> Result<()> {
        let namespace_name = interface_namespace_name(&resolve, &id, false);
        self.interface_names.insert(id, namespace_name.clone());
        let mut gen = self.interface(resolve, false, Some(namespace_name.to_string()));
        gen.interface = Some((id, name));

        for (name, ty) in &resolve.interfaces[id].types {
            gen.define_type(name, *ty);
        }

        for (_name, func) in resolve.interfaces[id].functions.iter() {
            if func.kind == FunctionKind::Freestanding {
                gen.export(func, Some(name));
            }
        }

        let object_body =  &gen.src.as_mut_string();
        let private_top_level_body = &gen.private_top_level_src.as_mut_string();
        let exports_stubs_body = &gen.export_stubs_src.as_mut_string();

        // TODO(Kotlin): Naming of exports
        uwriteln!(self.src, "interface {namespace_name} {{\n{object_body}\n}}\n");
        uwriteln!(self.export_stubs_src, "object {namespace_name}Impl : {namespace_name} {{\n{exports_stubs_body}\n}}\n");

        // if self.opts.generate_stubs {
        //     let mut gen = self.interface(resolve, false, Some(namespace_name.to_string()));
        //     gen.interface = Some((id, name));
        //     for (_name, func) in resolve.interfaces[id].functions.iter() {
        //         if func.kind == FunctionKind::Freestanding {
        //             let sig = gen.kotlin_signature(func);
        //             uwriteln!(gen.export_stubs_src, "{sig} {{ TODO() }}");
        //         }
        //     }
        //     // TODO: Handle resources
        //
        //     // TODO: Generate in a separate file
        //     let object_body =  &gen.src.as_mut_string();
        //     uwriteln!(self.export_stubs_src, "object {namespace_name}Impl {{\n{object_body}\n}}\n");
        // }


        uwriteln!(self.private_src, "{private_top_level_body}\n");
        Ok(())
    }

    fn import_funcs(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) {
        let name = &resolve.worlds[world].name;
        let interface_name = "__WorldImports".to_string();
        let mut gen = self.interface(resolve, true, Some(interface_name.clone()));

        for (i, (_name, func)) in funcs.iter().enumerate() {
            if i == 0 {
                uwriteln!(gen.src, "\n// import_funcs: Imported Functions from `{name}`");
            }
            gen.import(func, None);
        }

        uwriteln!(gen.gen.src, "object {interface_name} {{");
        gen.gen.src.push_str(&gen.src);
        uwriteln!(gen.gen.src, "\n}}\n");
        gen.gen.private_src.push_str(&gen.private_top_level_src);
    }

    fn export_funcs(
        &mut self,
        resolve: &Resolve,
        world: WorldId,
        funcs: &[(&str, &Function)],
        _files: &mut Files,
    ) -> Result<()> {
        let name = &resolve.worlds[world].name;
        let interface_name = "__WorldExports".to_string();

        let mut gen = self.interface(resolve, false, Some(interface_name.clone()));

        for (i, (_name, func)) in funcs.iter().enumerate() {
            if i == 0 {
                uwriteln!(gen.src, "\n// Exported Functions from `{name}`");
            }
            gen.export(func, None);
        }

        uwriteln!(gen.gen.src, "interface {interface_name} {{");
        gen.gen.src.push_str(&gen.src);
        uwriteln!(gen.gen.src, "\n}}\n");
        gen.gen.private_src.push_str(&gen.private_top_level_src);

        let exports_stubs_body = &gen.export_stubs_src.as_mut_string();

        uwriteln!(gen.gen.export_stubs_src, "object {interface_name}Impl : {interface_name} {{\n{exports_stubs_body}\n}}\n");

        // if generate_stubs {
        //     uwriteln!(self.src, "object {interface_name}Impl {{");
        //
        //     let mut gen = self.interface(resolve, false, Some(interface_name.clone()));
        //     for (_name, func) in funcs.iter() {
        //         let sig = gen.kotlin_signature(func);
        //         uwriteln!(gen.gen.export_stubs_src, "{sig} = TODO()\n");
        //     }
        //
        //     // TODO: Generate in a separate file
        //     let object_body =  &gen.export_stubs_src.as_mut_string();
        // }

        Ok(())
    }

    fn import_types(
        &mut self,
        resolve: &Resolve,
        _world: WorldId,
        types: &[(&str, TypeId)],
        _files: &mut Files,
    ) {
        let mut gen = self.interface(resolve, true, None);
        for (name, ty) in types {
            gen.define_type(name, *ty);
        }
        gen.gen.src.push_str(&gen.src);
    }

    fn finish(&mut self, resolve: &Resolve, id: WorldId, files: &mut Files) -> Result<()> {
        let world = &resolve.worlds[id];
        let snake = world.name.to_upper_camel_case();

        let version = env!("CARGO_PKG_VERSION");

        let mut support_kt_str = Source::default();
        uwriteln!(support_kt_str,
            "
            @file:OptIn(UnsafeWasmMemoryApi::class)

            import kotlin.wasm.unsafe.*
            class ComponentException(val value: Any?) : Throwable()

            sealed interface Option<out T> {{
                data class Some<T2>(val value: T2) : Option<T2>
                data object None : Option<Nothing>
            }}

            internal value class ResourceHandle(internal val value: Int)

            @WasmExport
            fun cabi_realloc(ptr: Int, oldSize: Int, align: Int, newSize: Int): Int =
                componentModelRealloc(ptr, oldSize, newSize)

            fun MemoryAllocator.STRING_TO_MEM(s: String): Int =
                writeToLinearMemory(s.encodeToByteArray()).address.toInt()

            fun STRING_FROM_MEM(addr: Int, len: Int): String =
                loadByteArray(addr.ptr, len).decodeToString()

            fun MALLOC(size: Int, align: Int): Int = TODO()

            val Int.ptr: Pointer
                get() = Pointer(this.toUInt())

            fun Pointer.loadUByte(): UByte = loadByte().toUByte()
            fun Pointer.loadUShort(): UShort = loadShort().toUShort()
            fun Pointer.loadUInt(): UInt = loadInt().toUInt()
            fun Pointer.loadULong(): ULong = loadLong().toULong()

            internal fun MemoryAllocator.writeToLinearMemory(value: String): Pointer =
                writeToLinearMemory(value.encodeToByteArray())

            internal fun loadString(addr: Pointer, size: Int): String =
                loadByteArray(addr, size).decodeToString()
            internal fun loadByteArray(addr: Pointer, size: Int): ByteArray =
                ByteArray(size) {{ i -> (addr + i).loadByte() }}
            internal fun MemoryAllocator.writeToLinearMemory(array: ByteArray): Pointer {{
                val pointer = allocate(array.size)
                var currentPointer = pointer
                array.forEach {{
                    currentPointer.storeByte(it)
                    currentPointer += 1
                }}
                return pointer
            }}


            fun Pointer.loadFloat(): Float = Float.fromBits(loadInt())
            fun Pointer.loadDouble(): Double = Double.fromBits(loadLong())
            fun Pointer.storeFloat(value: Float) {{ storeInt(value.toRawBits()) }}
            fun Pointer.storeDouble(value: Double) {{ storeLong(value.toRawBits()) }}

            internal object RepTable {{
                private val list = mutableListOf<Any>();
                private var firstVacant: Int? = null
                private data class Vacant(var next: Int?)

                fun add(v: Any): Int {{
                    val rep: Int
                    if (firstVacant != null) {{
                        rep = firstVacant!!
                        firstVacant = (list[rep] as Vacant).next
                        list[rep] = v
                    }} else {{
                        rep = list.size
                        list += v;
                    }}
                    return rep
                }}

                fun get(rep: Int): Any {{
                    check(list[rep] !is Vacant)
                    return list[rep];
                }}

                fun remove(rep: Int): Any {{
                    val v = get(rep)
                    list[rep] = Vacant(firstVacant)
                    firstVacant = rep
                    return v
                }}

                override fun toString(): String {{
                    return \"RepTable(firstVacant=${{firstVacant}}, list = $list)\"
                }}
            }}
            "
        );

        let mut tuple_counts = Vec::from_iter(&self.tuple_counts);
        tuple_counts.sort();

        for tup_size in tuple_counts {
            uwrite!(support_kt_str, "data class Tuple{tup_size}<");
            for i in 0..*tup_size {
                uwrite!(support_kt_str, "T{i},");
            }
            uwrite!(support_kt_str, ">(");
            for i in 0..*tup_size {
                uwrite!(support_kt_str, "val f{i}: T{i},");
            }
            uwriteln!(support_kt_str, ")");
        }
        files.push(&format!("ComponentSupport.kt"), support_kt_str.as_bytes());

        let mut kt_str = Source::default();
        wit_bindgen_core::generated_preamble(&mut kt_str, version);

        uwriteln!(kt_str,
            "
            @file:OptIn(UnsafeWasmMemoryApi::class)
            import kotlin.wasm.unsafe.*
            "
        );
        kt_str.push_str(&self.src);

        // TODO(Kotlin): Add custom section
        files.push(&format!("{snake}.kt"), kt_str.as_bytes());

        let mut private_kt_str = Source::default();
        wit_bindgen_core::generated_preamble(&mut private_kt_str, version);

        uwriteln!(private_kt_str,
            "
            @file:OptIn(UnsafeWasmMemoryApi::class)
            import kotlin.wasm.unsafe.*
            "
        );
        private_kt_str.push_str(&self.private_src);
        files.push(&format!("Internal{snake}.kt"), private_kt_str.as_bytes());

        if self.opts.generate_stubs {
            let mut stubs_kt = Source::default();
            wit_bindgen_core::generated_preamble(&mut stubs_kt, version);
            stubs_kt.push_str(&self.export_stubs_src);
            files.push(&format!("{snake}Impl.kt"), stubs_kt.as_bytes());
        }

        Ok(())
    }
}

impl Kotlin {
    fn interface<'a>(
        &'a mut self,
        resolve: &'a Resolve,
        in_import: bool,
        namespace_name: Option<String>,
    ) -> InterfaceGenerator<'a> {
        InterfaceGenerator {
            src: Source::default(),
            private_top_level_src: Source::default(),
            export_stubs_src: Source::default(),
            gen: self,
            resolve,
            interface: None,
            in_import: in_import,
            namespace_name
        }
    }
}

pub fn interface_namespace_name(resolve: &Resolve, id: &InterfaceId, in_imports: bool) -> String {
    let name = match &resolve.interfaces[*id].name {
        None => "AnonymousInterface".to_string(),
        Some(name) => name.to_upper_camel_case()
    };

    if in_imports {
        name
    } else {
        format!("{name}Exports")
    }
}

struct InterfaceGenerator<'a> {
    src: Source,
    private_top_level_src: Source,
    export_stubs_src: Source,
    in_import: bool,
    gen: &'a mut Kotlin,
    resolve: &'a Resolve,
    interface: Option<(InterfaceId, &'a WorldKey)>,
    namespace_name: Option<String>,
}

impl<'a> wit_bindgen_core::InterfaceGenerator<'a> for InterfaceGenerator<'a> {
    fn resolve(&self) -> &'a Resolve {
        self.resolve
    }

    fn type_record(&mut self, _id: TypeId, name: &str, record: &Record, docs: &Docs) {
        self.src.push_str("\n");
        self.src.push_str(kdoc(docs).as_str());
        self.src.push_str("data class ");
        let name = name.to_upper_camel_case();
        self.src.push_str(&name);
        // TODO(Kotlin): ident doesn't work
        self.src.push_str("(\n");
        for field in record.fields.iter() {
            self.src.push_str(kdoc(&field.docs).as_str());
            self.src.push_str("var ");
            self.src.push_str(&to_kotlin_ident(&field.name));
            self.src.push_str(": ");
            let ty = &field.ty;
            self.src.push_str(self.type_name(ty).as_str());
            self.src.push_str(",\n");
        }
        self.src.push_str(")\n");
    }

    fn type_resource(&mut self, type_id: TypeId, name: &str, docs: &Docs) {

        if !self.in_import {
            self.gen.exported_resources.insert(type_id);
        }

        let camel = name.to_upper_camel_case();

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
                @WasmImport("{import_module}", "[resource-drop]{name}")
                internal external fun {imported_function_prefix}_drop(handle: Int): Unit
            "#
        ));

        if !self.in_import {
            self.private_top_level_src.push_str(&format!(
                r#"
                    @WasmImport("{import_module}", "[resource-new]{name}")
                    internal external fun {imported_function_prefix}_new(handle: Int): Int

                    @WasmImport("{import_module}", "[resource-rep]{name}")
                    internal external fun {imported_function_prefix}_rep(handle: Int): Int
                "#
            ));
        }

        self.src.push_str(kdoc(docs).as_str());
        if !self.in_import {
            uwrite!(self.src, "abstract ")
        }

        uwriteln!(self.src, "class {camel} : AutoCloseable {{");
        uwriteln!(self.src, "internal var __handle: ResourceHandle = ResourceHandle(0)");

        if self.in_import { // Exported constructor handle
            uwriteln!(self.src, "internal constructor(handle: ResourceHandle) {{ __handle = handle }}");
        }

        // TODO: Zero out the handle
        uwriteln!(self.src, "override fun close() {{ {imported_function_prefix}_drop(__handle.value) }} ");

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
            uwriteln!(self.src, "interface Statics {{");
            uwriteln!(self.export_stubs_src, "companion object : Statics {{");
        } else {
            uwriteln!(self.src, "companion object {{");
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
        self.src.push_str("}");
        self.src.push_str("}");

        if !self.in_import {
            self.export_stubs_src.push_str("}");
            self.export_stubs_src.push_str("}");
        }
    }

    fn type_flags(&mut self, _id: TypeId, name: &str, flags: &Flags, docs: &Docs) {
        self.src.push_str("\n");
        self.src.push_str(kdoc(docs).as_str());
        self.src.push_str("value class ");
        let name = name.to_upper_camel_case();
        self.src.push_str(&name);
        // TODO(Kotlin): Support underlying values smaller than Long
        self.src.push_str(" internal constructor(val _value: Long) {\n");
        self.src.push_str("constructor(\n");
        for flag in flags.flags.iter() {
            uwrite!(
                self.src,
                "{}: Boolean = false,",
                flag.name.to_lower_camel_case(),
            );
        }
        self.src.push_str("\n) : this(0L");
        for (i, flag) in flags.flags.iter().enumerate() {
            uwrite!(
                self.src,
                " or (if ({}) (1L shl {i}) else 0L)",
                flag.name.to_lower_camel_case(),
            );
        }
        self.src.push_str(")\n");
        for (i, flag) in flags.flags.iter().enumerate() {
            uwriteln!(
                self.src,
                "val {}: Boolean get() = (_value and (1L shl {i})) != 0L",
                flag.name.to_lower_camel_case(),
            );
        }
        // TODO(Kotlin): Add toString method

        self.src.push_str("}\n");
    }

    fn type_variant(&mut self, _id: TypeId, name: &str, variant: &Variant, docs: &Docs) {
        self.src.push_str("\n");
        self.src.push_str(kdoc(docs).as_str());
        self.src.push_str("sealed interface ");
        let variant_name = name.to_upper_camel_case();
        self.src.push_str(&variant_name);
        self.src.push_str("{ \n");
        for case in &variant.cases {
            let case_name = case.name.to_upper_camel_case();
            match &case.ty {
                None => {
                    self.src.push_str("data object ");
                    self.src.push_str(case_name.as_str());
                }
                Some(ty) => {
                    self.src.push_str("data class ");
                    self.src.push_str(case_name.as_str());
                    self.src.push_str("(val value: ");
                    self.src.push_str(self.type_name(ty).as_str());
                    self.src.push_str(")");
                }
            }
            self.src.push_str(" : ");
            self.src.push_str(&variant_name);
            self.src.push_str("\n");
        }
        self.src.push_str("}");
    }

    fn type_enum(&mut self, _id: TypeId, name: &str, enum_: &Enum, docs: &Docs) {
        uwrite!(self.src, "\n");
        self.src.push_str(kdoc(docs).as_str());
        self.src.push_str("enum class ");
        let name = name.to_upper_camel_case();
        self.src.push_str(&name);
        self.src.push_str(" {\n");
        for case in enum_.cases.iter() {
            self.src.push_str(kdoc(&case.docs).as_str());
            self.src.push_str(case.name.to_shouty_snake_case().as_str());
            self.src.push_str(",\n");
        }
        self.src.push_str("}\n");
    }

    // Kotlin does not support nested type aliases yet, issue: https://youtrack.jetbrains.com/issue/KT-45285
    fn type_alias(&mut self, _id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {}
    fn type_tuple(&mut self, _id: TypeId, _name: &str, _tuple: &Tuple, _docs: &Docs) {}
    fn type_option(&mut self, _id: TypeId, _name: &str, _payload: &Type, _docs: &Docs) {}
    fn type_result(&mut self, _id: TypeId, _name: &str, _result: &Result_, _docs: &Docs) {}
    fn type_list(&mut self, _id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {}
    fn type_builtin(&mut self, _id: TypeId, _name: &str, _ty: &Type, _docs: &Docs) {}
}

impl InterfaceGenerator<'_> {
    fn resource_import_prefix(&self, id: &TypeId) -> String {
        let mut result = String::new();
        let is_exported = self.gen.exported_resources.contains(&id);
        let ty = &self.resolve.types[*id];

        match &ty.owner {
            TypeOwner::Interface(ty_interface_id) => {
                let namespace_name = &self.gen.interface_names[ty_interface_id];
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

        match &ty.name {
            None => {}
            Some(name) => {
                let name = name.to_upper_camel_case();
                uwrite!(result, "_{name}");
            }
        }

        let common_prefix = "__cm_resource_abi";
        return if is_exported {
            format!("{common_prefix}_export_{result}")
        } else {
            format!("{common_prefix}_import_{result}")
        };
    }

    fn type_name(&self, ty: &Type) -> String {
        let mut name = String::new();
        self.push_type_name(ty, &mut name);
        name
    }

    fn type_id_name(&self, id: &TypeId) -> String {
        let mut name = String::new();
        self.push_type_id_name(id, &mut name);
        name
    }

    fn push_type_name(&self, ty: &Type, dst: &mut String) {
        match ty {
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
            Type::Id(id) => self.push_type_id_name(id, dst)
        }
    }

    fn push_type_id_name(&self, id: &TypeId, dst: &mut String) {
        let ty = &self.resolve.types[*id];
        match &ty.kind {
            TypeDefKind::Type(t) => self.push_type_name(t, dst),
            TypeDefKind::Record(_)
            | TypeDefKind::Resource
            | TypeDefKind::Flags(_)
            | TypeDefKind::Enum(_)
            | TypeDefKind::Variant(_) => {
                let is_exported_resource = self.gen.exported_resources.contains(id);
                match &ty.owner {
                    TypeOwner::Interface(ty_interface_id) => {
                        let namespace_name = &self.gen.interface_names[ty_interface_id];
                        if is_exported_resource {
                            uwrite!(dst, "{namespace_name}Impl.");  // Exported resources live only in Implementation namespace
                        } else {
                            uwrite!(dst, "{namespace_name}.");
                        }
                    }
                    TypeOwner::World(_) => {
                        // TODO(KT): World fqn
                    }
                    TypeOwner::None => {}
                }

                if let Some(name) = &ty.name {
                    dst.push_str(&name.to_upper_camel_case());
                    if is_exported_resource {
                        dst.push_str("Impl")
                    }
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
                if is_option_type(self.resolve, ty) {
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
        }
    }

    fn kotlin_fun_name(&self, func: &Function) -> String {
        to_kotlin_ident(func.item_name())
    }

    fn import(&mut self, func: &Function, interface_name: Option<&WorldKey>) {
        let sig = self.resolve.wasm_signature(AbiVariant::GuestImport, func);
        self.private_top_level_src.push_str("\n");

        uwriteln!(
            self.private_top_level_src,
            "@WasmImport(\"{}\", \"{}\")",
            match interface_name {
                Some(name) => self.resolve.name_world_key(name),
                None => "\\$root".to_string(),  // TODO(Kotlin): Escape all strings properly
            },
            func.name
        );
        let name = self.kotlin_fun_name(func);
        let import_name = self.gen.names.tmp(&format!("__wasm_import_{name}",));
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

        self.src.push_str(kdoc(&func.docs).as_str());
        self.src.push_str("public ");
        {
            let sig = self.kotlin_signature(func);
            self.src.push_str(sig.as_str());
            self.src.push_str("\n");

        }
        if let FunctionKind::Constructor(_) = func.kind {
            // IIFE in primary construct call
            self.src.push_str(": this(ResourceHandle(run(fun (): Int");
        }
        self.src.push_str(" {\n");
        self.src.push_str("// <editor-fold defaultstate=\"collapsed\" desc=\"Generated Bindings Code\">\n");

        self.src.push_str(" withScopedMemoryAllocator { allocator -> \n");

        let mut f = FunctionBindgen::new(self, &import_name, func.kind.clone());
        for (idx, (name, _)) in func.params.iter().enumerate() {
            let param = if idx == 0 && matches!(func.kind, FunctionKind::Method(_)) {
                "this".to_string()
            } else {
                to_kotlin_ident(name)
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

        self.src.push_str(&String::from(src));
        self.src.push_str("}\n");
        self.src.push_str("// </editor-fold>\n");
        self.src.push_str("}\n");
        if let FunctionKind::Constructor(_) = func.kind {
            // End of IIFE in primary construct call
            self.src.push_str(")))\n");
        }
    }

    fn export(&mut self, func: &Function, interface_name: Option<&WorldKey>) {
        let wasm_sig = self.resolve.wasm_signature(AbiVariant::GuestExport, func);

        let core_module_name = interface_name.map(|s| self.resolve.name_world_key(s));
        let export_name = func.core_export_name(core_module_name.as_deref());
        {
            let kotlin_sig = self.kotlin_signature(func);
            if !matches!(func.kind, FunctionKind::Constructor(_)) {  // Constructor in exported abstract resource class is not needed
                uwriteln!(self.src, "abstract {kotlin_sig}");
                uwriteln!(self.export_stubs_src, "override {kotlin_sig} {{ TODO() }}");
            } else {
                uwriteln!(self.export_stubs_src, "{kotlin_sig} : super() {{ TODO() }}");
            }
        }

        uwriteln!(
            self.private_top_level_src,
            "\n@WasmExport(\"{export_name}\")"
        );
        let name = self.kotlin_fun_name(func);
        let export_fun_name = self.gen.names.tmp(&format!("__wasm_export_{name}"));

        let mut f = FunctionBindgen::new(self, &export_fun_name, func.kind.clone());
        let s: &mut Source = &mut f.gen.private_top_level_src;
        s.push_str("fun ");
        s.push_str(&export_fun_name);
        s.push_str("(");
        for (i, param) in wasm_sig.params.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let name = format!("p{i}");

            uwrite!(s, "{name}: ");
            s.push_str(wasm_type(*param));
            f.params.push(name);
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


        // Perform all lifting/lowering and append it to our src.
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

    fn kotlin_signature(&mut self, func: &Function) -> String {
        let mut result = String::new();

        let name = self.kotlin_fun_name(func);
        if let FunctionKind::Constructor(_) = func.kind {
            result.push_str("constructor");
        } else {
            result.push_str("fun ");
            result.push_str(&name);
        }
        result.push_str("(");
        for (i, (name, ty)) in func.params.iter().enumerate() {
            if let FunctionKind::Method(_) = func.kind {
                if i == 0 { continue }
                if i > 1 { result.push_str(", "); }
            } else {
                if i > 0 { result.push_str(", "); }
            }
            result.push_str(&to_kotlin_ident(name));
            result.push_str(": ");
            result.push_str(self.type_name(ty).as_str());
        }
        result.push_str(")");
        if let FunctionKind::Constructor(_) = func.kind {
            return result;
        }

        result.push_str(": ");
        match &func.results {
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
        }
        result
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

struct FunctionBindgen<'a, 'b> {
    gen: &'a mut InterfaceGenerator<'b>,
    kind: FunctionKind,
    locals: Ns,
    src: Source,
    func_to_call: &'a str,
    block_storage: Vec<Source>,
    blocks: Vec<(String, Vec<String>)>,
    payloads: Vec<String>,
    params: Vec<String>,
}

impl<'a, 'b> FunctionBindgen<'a, 'b> {
    fn new(
        gen: &'a mut InterfaceGenerator<'b>,
        func_to_call: &'a str,
        kind: FunctionKind,
    ) -> FunctionBindgen<'a, 'b> {
        FunctionBindgen {
            gen,
            kind,
            locals: Default::default(),
            src: Default::default(),
            func_to_call,
            block_storage: Vec::new(),
            blocks: Vec::new(),
            payloads: Vec::new(),
            params: Vec::new(),
        }
    }

    fn load(&mut self, ty: &str, offset: i32, operands: &[String], results: &mut Vec<String>) {
        results.push(format!("({} + {offset}).ptr.load{ty}()", operands[0]));
    }

    fn load_ext(&mut self, ty: &str, offset: i32, operands: &[String], results: &mut Vec<String>) {
        self.load(ty, offset, operands, results);
        let result = results.pop().unwrap();
        results.push(format!("{}.toInt()", result));
    }

    fn store_impl(&mut self, ty: &str, offset: i32, address: &String, value: &String) {
        uwriteln!(self.src, "({address} + {offset}).ptr.store{ty}({value})");
    }

    fn store(&mut self, ty: &str, offset: i32, operands: &[String]) {
        self.store_impl(ty, offset, &operands[1], &operands[0])
    }

    fn store_converted(&mut self, ty: &str, offset: i32, operands: &[String]) {
        let converted_value = format!("{}.to{ty}()", operands[0]);
        self.store_impl(ty, offset, &operands[1], &converted_value)
    }
}

impl Bindgen for FunctionBindgen<'_, '_> {
    type Operand = String;

    fn emit(
        &mut self,
        _resolve: &Resolve,
        inst: &Instruction<'_>,
        operands: &mut Vec<String>,
        results: &mut Vec<String>,
    ) {
        match inst {
            Instruction::GetArg { nth } => results.push(self.params[*nth].clone()),
            Instruction::I32Const { val } => results.push(val.to_string()),
            Instruction::ConstZero { tys } => {
                for ty in tys.iter() {
                    results.push(
                        match ty {
                            WasmType::I32 => "0",
                            WasmType::I64 => "0L",
                            WasmType::F32 => "0.0f",
                            WasmType::F64 => "0.0",
                            WasmType::Pointer => "0",
                            WasmType::PointerOrI64 => "0L",
                            WasmType::Length => "0",
                        }.to_string()
                    );
                }
            }

            Instruction::U8FromI32 => results.push(format!("{}.toUByte()", operands[0])),
            Instruction::S8FromI32 => results.push(format!("{}.toByte()", operands[0])),
            Instruction::U16FromI32 => results.push(format!("{}.toUShort()", operands[0])),
            Instruction::S16FromI32 => results.push(format!("{}.toShort()", operands[0])),
            Instruction::U32FromI32 => results.push(format!("{}.toUInt()", operands[0])),
            Instruction::S32FromI32 | Instruction::S64FromI64 => results.push(operands[0].clone()),
            Instruction::U64FromI64 => results.push(format!("{}.toULong()", operands[0])),

            Instruction::I32FromU8
            | Instruction::I32FromS8
            | Instruction::I32FromU16
            | Instruction::I32FromS16
            | Instruction::I32FromU32 => {
                results.push(format!("{}.toInt()", operands[0]));
            }
            Instruction::I32FromS32 | Instruction::I64FromS64 => results.push(operands[0].clone()),
            Instruction::I64FromU64 => {
                results.push(format!("{}.toLong()", operands[0]));
            }

            Instruction::CoreF32FromF32
            | Instruction::CoreF64FromF64
            | Instruction::F32FromCoreF32
            | Instruction::F64FromCoreF64 => {
                results.push(operands[0].clone());
            }

            // TODO(Kotlin): Do we need a different representation for Char?
            Instruction::CharFromI32
            | Instruction::I32FromChar => results.push(operands[0].clone()),

            Instruction::Bitcasts { casts } => {
                for (cast, op) in casts.iter().zip(operands) {
                    results.push(perform_cast(op, cast));
                }
            }

            Instruction::BoolFromI32 => results.push(format!("({} != 0)", operands[0])),
            Instruction::I32FromBool => results.push(format!("(if({}) 1 else 0)", operands[0])),

            Instruction::RecordLower { record, .. } => {
                let op = &operands[0];
                for f in record.fields.iter() {
                    results.push(format!("{op}.{}", to_kotlin_ident(&f.name)));
                }
            }

            Instruction::RecordLift { ty, .. } => {
                let name = self.gen.type_name(&Type::Id(*ty));
                let mut result = format!("{name}(\n");
                for op in operands {
                    uwriteln!(result, "{},", op);
                }
                result.push_str(")");
                results.push(result);
            }

            Instruction::TupleLift { tuple, ty } => {
                match tuple.types.len() {
                    0 => {
                        results.push("Unit".to_string());
                    }
                    1 => {
                        results.push(operands[0].clone());
                    }
                    count => {
                        let name = self.gen.type_name(&Type::Id(*ty));
                        self.gen.gen.tuple_counts.insert(count);
                        let mut result = format!("{name}(\n");
                        for op in operands {
                            uwriteln!(result, "{},", op);
                        }
                        result.push_str(")");
                        results.push(result);
                    }
                }
            }

            Instruction::TupleLower { tuple, .. } => {
                let op = &operands[0];
                match tuple.types.len() {
                    0 => {}
                    1 => {
                        results.push(op.clone());
                    }
                    2 => {
                        results.push(format!("{op}.first"));
                        results.push(format!("{op}.second"));
                    }
                    3 => {
                        results.push(format!("{op}.first"));
                        results.push(format!("{op}.second"));
                        results.push(format!("{op}.third"));
                    }
                    len => {
                        self.gen.gen.tuple_counts.insert(len);
                        for i in 0..len {
                            results.push(format!("{op}.f{i}"));
                        }
                    }
                }
            }

            Instruction::HandleLower {
                handle,
                ..
            } => {
                let (Handle::Own(ty) | Handle::Borrow(ty)) = handle;
                let is_own = matches!(handle, Handle::Own(_));
                let handle = self.locals.tmp("handle");
                let id = dealias(self.gen.resolve, *ty);
                let imported_function_prefix = self.gen.resource_import_prefix(&id);
                let is_exported = self.gen.gen.exported_resources.contains(&id);
                let op = &operands[0];
                uwriteln!(self.src, "var {handle} = {op}.__handle.value;");

                if is_exported {
                    let local_rep = self.locals.tmp("localRep");
                    uwriteln!(
                        self.src,
                        "if ({handle} == 0) {{
                             var {local_rep} = RepTable.add({op});
                             {handle} = {imported_function_prefix}_new({local_rep});
                         }}
                         "
                    );
                }

                if is_own {
                    uwriteln!(self.src, "{op}.__handle = ResourceHandle(0);");
                }

                results.push(format!("{handle}"))
            }

            Instruction::HandleLift { handle, .. } => {
                let (Handle::Own(ty) | Handle::Borrow(ty)) = handle;
                let is_own = matches!(handle, Handle::Own(_));
                let resource = self.locals.tmp("resource");
                let id = dealias(self.gen.resolve, *ty);
                let imported_function_prefix = self.gen.resource_import_prefix(&id);
                let is_exported = self.gen.gen.exported_resources.contains(&id);
                let op = &operands[0];
                let resource_type_name = self.gen.type_name(&Type::Id(*ty));

                if is_exported {
                    if is_own {
                        uwriteln!(self.src,
                            "val {resource} = RepTable.get({imported_function_prefix}_rep({op})) as {resource_type_name}
                                 {resource}.__handle = ResourceHandle({op})
                            "
                        );
                    } else {
                        uwriteln!(self.src, "val {resource} = RepTable.get({op}) as {resource_type_name}");
                    }
                } else {
                    if let FunctionKind::Constructor(_) = self.kind {
                        // Imported CM constructor return raw handle, it
                        // is then wrapped inside generated class constructor
                        uwriteln!(self.src, "val {resource} = {op}");
                    } else {
                        uwriteln!(self.src, "val {resource} = {resource_type_name}(ResourceHandle({op}))")
                        // TODO: Drop this resource at the end of the function
                    }
                }
                results.push(resource);
            },

            Instruction::FlagsLower { flags, .. } => {
                let value = format!("{}._value", operands[0]);
                match flags.repr() {
                    FlagsRepr::U8
                    | FlagsRepr::U16
                    | FlagsRepr::U32(1) => {
                        results.push(format!("{value}.toInt()"))
                    }
                    FlagsRepr::U32(2) => {
                            let tmp = self.locals.tmp("flags");
                            uwriteln!(self.src, "val {tmp} = {value}");
                            results.push(format!("{tmp}.toInt()"));
                            results.push(format!("({tmp} ushr 32).toInt()"));
                    }
                    FlagsRepr::U32(size) => {
                        unimplemented!("sizes more than 2 are not supported for FlagsRepr::U32(size={size})")
                    }
                }
            }
            ,

            Instruction::FlagsLift { flags, ty, .. } => {
                let class_name = self.gen.type_name(&Type::Id(*ty));
                let op0 = &operands[0];
                match flags.repr() {
                    FlagsRepr::U8
                    | FlagsRepr::U16
                    | FlagsRepr::U32(1) => {
                        results.push(format!("{class_name}({op0}.toLong())"))
                    }

                    FlagsRepr::U32(2) => {
                        let op1 = &operands[1];
                        results.push(format!("{class_name}(({op0}.toLong() and 0xffffffffL) or ({op1}.toLong() shl 32))"));
                    }
                    FlagsRepr::U32(size) => {
                        unimplemented!("sizes more than 2 are not supported for FlagsRepr::U32(size={size})")
                    }
                }
            },

            Instruction::VariantPayloadName => {
                let payload = self.locals.tmp("payload");
                results.push(payload.clone());
                self.payloads.push(payload);
            }

            Instruction::VariantLower {
                variant,
                results: result_types,
                ty,
                ..
            } => {
                self.src.push_str("// VariantLower START\n");

                let blocks = self
                    .blocks
                    .drain(self.blocks.len() - variant.cases.len()..)
                    .collect::<Vec<_>>();
                let payloads = self
                    .payloads
                    .drain(self.payloads.len() - variant.cases.len()..)
                    .collect::<Vec<_>>();

                let mut variant_results = Vec::with_capacity(result_types.len());
                for ty in result_types.iter() {
                    let name = self.locals.tmp("variant");
                    let wasm_type_name = wasm_type(*ty);
                    results.push(name.clone());
                    uwriteln!(self.src, "val {name}: {wasm_type_name}");
                    variant_results.push(name);
                }

                let op0 = &operands[0];

                let op0_name = self.locals.tmp("x");

                let variant_class_name = self.gen.type_id_name(ty);

                uwriteln!(self.src, "when (val {op0_name} = {op0}) {{");
                for ((case, (block, block_results)), payload) in
                    variant.cases.iter().zip(blocks).zip(payloads)
                {
                    let case_class_name = case.name.to_upper_camel_case();
                    uwriteln!(self.src, "is {variant_class_name}.{case_class_name} -> {{");
                    if case.ty.is_some() {
                        uwriteln!(self.src, "val {payload} = {op0_name}.value");
                    }
                    self.src.push_str(&block);
                    for (name, result) in variant_results.iter().zip(&block_results) {
                        uwriteln!(self.src, "{} = {};", name, result);
                    }
                    self.src.push_str("}\n");
                }
                self.src.push_str("else -> error(\"unreachable\")\n");
                self.src.push_str("}\n");
                self.src.push_str("// VariantLower END\n");
            }

            Instruction::VariantLift { variant,  ty, .. } => {
                self.src.push_str("// VariantLift START.\n");

                let blocks = self
                    .blocks
                    .drain(self.blocks.len() - variant.cases.len()..)
                    .collect::<Vec<_>>();

                let variant_class_name = self.gen.type_id_name(ty);
                let result = self.locals.tmp("variant");
                let op0 = &operands[0];
                uwriteln!(self.src, "val {result} = when ({op0}) {{");
                for (i, (case, (block, block_results))) in
                    variant.cases.iter().zip(blocks).enumerate()
                {
                    let case_class_name = case.name.to_upper_camel_case();
                    let case_class_qualified_name = format!("{variant_class_name}.{case_class_name}");

                    uwriteln!(self.src, "{} -> {{", i);
                    self.src.push_str(&block);
                    match case.ty {
                        None => {  // data object case
                            assert_eq!(block_results.len(), 0);
                            uwriteln!(self.src, "{case_class_qualified_name}")
                        }
                        Some(_) => {  // data class with single property case
                            assert_eq!(block_results.len(), 1);
                            let block_result = &block_results[0];
                            uwriteln!(self.src, "{case_class_qualified_name}({block_result})")
                        }
                    }
                    uwriteln!(self.src, "}}");
                }
                self.src.push_str("else -> error(\"unreachable\")\n");
                self.src.push_str("}\n");
                results.push(result);
                self.src.push_str("// VariantLift END\n");
            }

            Instruction::OptionLower {
                results: result_types,
                payload,
                ..
            } => {
                let (mut some, some_results) = self.blocks.pop().unwrap();
                let (mut none, none_results) = self.blocks.pop().unwrap();
                let some_payload = self.payloads.pop().unwrap();
                let _none_payload = self.payloads.pop().unwrap();

                for (i, ty) in result_types.iter().enumerate() {
                    let wasm_ty = wasm_type(*ty);
                    let name = self.locals.tmp("option");
                    results.push(name.clone());
                    uwriteln!(self.src, "val {name}: {wasm_ty}");
                    let some_result = &some_results[i];
                    uwriteln!(some, "{name} = {some_result}");
                    let none_result = &none_results[i];
                    uwriteln!(none, "{name} = {none_result}");
                }

                let op0 = &operands[0];

                let option_name = self.locals.tmp("option");
                if is_option_type(self.gen.resolve, payload) {
                    self.src.push_str(format!(
                        "\
                    val {option_name} = {op0}
                    if ({option_name} is Option.Some) {{
                        val {some_payload} = {option_name}.value
                        {some}}} else {{
                        {none}}}
                    "
                    ).as_str());
                } else {
                    self.src.push_str(format!(
                        "\
                    val {some_payload} = {op0}
                    if ({some_payload} != null) {{
                        {some}}} else {{
                        {none}}}
                    "
                    ).as_str());
                }
            }

            Instruction::OptionLift { payload, .. } => {
                self.src.push_str("// OptionLift start\n");
                let (some, some_results) = self.blocks.pop().unwrap();
                let (none, none_results) = self.blocks.pop().unwrap();
                assert_eq!(none_results.len(), 0);
                assert_eq!(some_results.len(), 1);

                let op0 = &operands[0];
                let some_result = &some_results[0];
                let result = self.locals.tmp("option");

                if is_option_type(self.gen.resolve, payload) {
                    uwriteln!(
                        self.src,
                        "val {result} = if ({op0} == 1) {{
                            {some} Option.Some({some_result})
                        }} else {{
                            {none} Option.None
                        }}"
                    );
                } else {
                    uwriteln!(
                        self.src,
                        "val {result} = if ({op0} == 1) {{
                            {some} {some_result}
                        }} else {{
                            {none} null
                        }}"
                    );
                }

                results.push(result);
                self.src.push_str("// OptionLift end\n");
            }

            Instruction::ResultLower {
                results: result_types,
                result,
                ..
            } => {
                let (mut err, err_results) = self.blocks.pop().unwrap();
                let (mut ok, ok_results) = self.blocks.pop().unwrap();
                let err_payload = self.payloads.pop().unwrap();
                let ok_payload = self.payloads.pop().unwrap();

                for (i, ty) in result_types.iter().enumerate() {
                    let wasm_ty = wasm_type(*ty);
                    let name = self.locals.tmp("result");
                    results.push(name.clone());

                    uwriteln!(self.src, "val {name}: {wasm_ty}");
                    let ok_result = &ok_results[i];
                    uwriteln!(ok, "{name} = {ok_result};");
                    let err_result = &err_results[i];
                    uwriteln!(err, "{name} = {err_result};");
                }

                let op0 = &operands[0];
                let bind_ok = if result.ok.is_some() {
                    format!("val {ok_payload} = {op0}.getOrThrow()!!\n")
                } else {
                    String::new()
                };
                let bind_err = if let Some(err_ty) = &result.err {
                    let err_kt_ty_name = self.gen.type_name(err_ty);
                    format!("val {err_payload} = ({op0}.exceptionOrNull() as ComponentException).value as {err_kt_ty_name}\n")
                } else {
                    String::new()
                };

                uwrite!(
                    self.src,
                    "\
                    if ({op0}.isFailure) {{
                        {bind_err}\
                        {err}\
                    }} else {{
                        {bind_ok}\
                        {ok}\
                    }}
                    "
                );
            }

            Instruction::ResultLift { result, ty, .. } => {
                let (mut err, err_results) = self.blocks.pop().unwrap();
                assert_eq!(err_results.len(), result.err.is_some() as usize);
                let (mut ok, ok_results) = self.blocks.pop().unwrap();
                assert_eq!(ok_results.len(), result.ok.is_some() as usize);

                if err.len() > 0 {
                    err.push_str("\n");
                }
                if ok.len() > 0 {
                    ok.push_str("\n");
                }

                let op0 = &operands[0];
                let result_tmp = self.locals.tmp("result");


                let kt_result_type: String = self.gen.type_name(&Type::Id(*ty)).clone();
                let ok_result = if result.ok.is_some() {
                    let ok_result = &ok_results[0];
                    format!("{kt_result_type}.success({ok_result})")
                } else {
                    format!("{kt_result_type}.success(Unit)")
                };

                let err_result = if let Some(_) = result.err.as_ref() {
                    let err_result = &err_results[0];
                    format!("{kt_result_type}.failure(ComponentException({err_result}))")
                } else {
                    format!("{kt_result_type}.failure(ComponentException(Unit))")
                };

                uwriteln!(
                    self.src,
                    "val {result_tmp} = if ({op0} == 0) {{
                        {ok} {ok_result}
                    }} else {{
                        {err} {err_result}
                    }}"
                );
                results.push(result_tmp);
            }

            Instruction::EnumLower { .. } => results.push(format!("{}.ordinal", operands[0])),
            Instruction::EnumLift { ty, .. } => {
                let op0 = &operands[0];
                let enum_class_name= self.gen.type_name(&Type::Id(*ty)).clone();
                results.push(format!("{enum_class_name}.values()[{op0}]"));
            },

            Instruction::ListCanonLower { .. } | Instruction::ListCanonLift { .. } => {
                unreachable!("Kotlin Lists are non-canonical")
            }

            Instruction::StringLower { .. } => {
                let op = &operands[0];
                let ptr = self.locals.tmp("ptr");
                let len = self.locals.tmp("len");
                let bytearray = self.locals.tmp("bytearray");

                // TODO(Kotlin): Post-return cleanup
                uwriteln!(
                    self.src,
                    "
                    val {bytearray} = {op}.encodeToByteArray()
                    val {len} = {bytearray}.size
                    val {ptr} = allocator.writeToLinearMemory({bytearray}).address.toInt()
                    "
                );

                results.push(format!("{ptr}"));
                results.push(format!("{len}"));
            }
            Instruction::StringLift { .. } => {
                let ptr = &operands[0];
                let len = &operands[1];
                results.push(format!("STRING_FROM_MEM({ptr}, {len})"));
            }

            Instruction::ListLower { element, .. } => {
                let (body, _) = self.blocks.pop().unwrap();

                let op = &operands[0];
                let size = self.gen.gen.sizes.size(element);
                let align = self.gen.gen.sizes.align(element);
                let address = self.locals.tmp("address");
                let index = self.locals.tmp("index");

                uwrite!(
                    self.src,
                    "
                    val {address} = allocator.allocate({op}.size * {size} /*, align={align}*/).address.toInt()
                    for (({index}, el) in {op}.withIndex()) {{
                        val base = {address} + ({index} * {size})
                        {body}
                    }}
                    "
                );

                results.push(address);
                results.push(format!("{op}.size"));
            }

            Instruction::ListLift { element, .. } => {
                let (body,block_results) = self.blocks.pop().unwrap();
                let address = &operands[0];
                let length = &operands[1];
                let list = self.locals.tmp("list");
                let ty = self.gen.type_name(element);
                let size = self.gen.gen.sizes.size(element);
                let index = self.locals.tmp("i");

                let result = &block_results[0];

                // TODO(Kotlin): Primitive array types
                uwrite!(
                    self.src,
                    "
                    val {list} = ArrayList<{ty}>({length})
                    for ({index} in 0 until {length}) {{
                        val base = ({address}) + ({index} * {size})
                        {body}
                        {list}.add({result})
                    }}
                    "
                );

                results.push(list);
            }
            // TODO(Kotlin): Reserve this names
            Instruction::IterElem { .. } => results.push("el".to_string()),
            Instruction::IterBasePointer => results.push("base".to_string()),

            Instruction::CallWasm { sig, .. } => {
                match sig.results.len() {
                    0 => {}
                    1 => {
                        let ret = self.locals.tmp("ret");
                        let ty = wasm_type(sig.results[0]);
                        uwrite!(self.src, "val {ret}: {ty} = ");
                        results.push(ret);
                    }
                    _ => unimplemented!(),
                }

                self.src.push_str(self.func_to_call);
                self.src.push_str("(");
                for (i, op) in operands.iter().enumerate() {
                    if i > 0 {
                        self.src.push_str(", ");
                    }
                    self.src.push_str(op);
                }
                self.src.push_str(")\n");
                self.src.push_str("freeAllComponentModelReallocAllocatedMemory();\n");
            }

            Instruction::CallInterface { func } => {
                let (assignment, destructure) = match func.results.len() {
                    0 => (String::new(), String::new()),
                    1 => {
                        let ty = self.gen.type_name(func.results.iter_types().next().unwrap());
                        let result = self.locals.tmp("result");
                        let assignment = format!("val {result}: {ty} = ");
                        results.push(result);
                        (assignment, String::new())
                    }
                    count => {
                        self.gen.gen.tuple_counts.insert(count);
                        let result = self.locals.tmp("result");
                        let assignment = format!("val {result} = ");

                        let destructure = func
                            .results
                            .iter_types()
                            .enumerate()
                            .map(|(index, ty)| {
                                let ty = self.gen.type_name(ty);
                                let my_result = self.locals.tmp("result");
                                let assignment = format!("val {my_result}: {ty} = {result}.f{index}");
                                results.push(my_result);
                                assignment
                            })
                            .collect::<Vec<_>>()
                            .join("\n");

                        (assignment, destructure)
                    }
                };

                let call_namespace = match &self.gen.namespace_name {
                    None => String::new(),
                    Some(name) => format!("{name}Impl.")
                };
                let name = self.gen.kotlin_fun_name(func);

                uwrite!(self.src, "{assignment}");
                match func.kind {
                    FunctionKind::Freestanding => {
                        let args = operands.join(", ");
                        uwriteln!(self.src, "{call_namespace}{name}({args})");
                    }
                    FunctionKind::Method(_) => {
                        let receiver_arg = operands[0].clone();
                        let regular_args = operands[1..].to_vec().join(", ");
                        uwriteln!(self.src, "{receiver_arg}.{name}({regular_args})");
                    }
                    FunctionKind::Static(resource_type) => {
                        let args = operands.join(", ");
                        let resource_class_name = self.gen.type_id_name(&resource_type);
                        uwriteln!(self.src, "{resource_class_name}.{name}({args})");
                    }
                    FunctionKind::Constructor(resource_type) => {
                        let resource_class_name = self.gen.type_id_name(&resource_type);
                        let args = operands.join(", ");
                        uwriteln!(self.src, "{resource_class_name}({args})");
                    }
                }
                uwriteln!(self.src, "{destructure}");
            }
            Instruction::Return { amt, .. } => {
                match *amt {
                    0 => (),
                    1 => uwriteln!(self.src, "return {}", operands[0]),
                    count => {
                        let results = operands.join(", ");
                        self.gen.gen.tuple_counts.insert(count);
                        uwriteln!(self.src, "return Tuple{count}({results})")
                    }
                }
            }

            Instruction::I32Load { offset } |
            Instruction::PointerLoad { offset } |
            Instruction::LengthLoad { offset } => self.load("Int", *offset, operands, results),
            Instruction::I64Load { offset } => self.load("Long", *offset, operands, results),
            Instruction::F32Load { offset } => self.load("Float", *offset, operands, results),
            Instruction::F64Load { offset } => self.load("Double", *offset, operands, results),
            Instruction::I32Load8U { offset } => self.load_ext("UByte", *offset, operands, results),
            Instruction::I32Load8S { offset } => self.load_ext("Byte", *offset, operands, results),
            Instruction::I32Load16U { offset } => self.load_ext("UShort", *offset, operands, results),
            Instruction::I32Load16S { offset } => self.load_ext("Short", *offset, operands, results),
            Instruction::I32Store { offset } |
            Instruction::PointerStore { offset } |
            Instruction::LengthStore { offset } => self.store("Int", *offset, operands),
            Instruction::I64Store { offset } => self.store("Long", *offset, operands),
            Instruction::F32Store { offset } => self.store("Float", *offset, operands),
            Instruction::F64Store { offset } => self.store("Double", *offset, operands),
            Instruction::I32Store8 { offset } => self.store_converted("Byte", *offset, operands),
            Instruction::I32Store16 { offset } => self.store_converted("Short", *offset, operands),

            Instruction::GuestDeallocate { .. } => {
                uwriteln!(self.src, "// GuestDeallocate({})", operands[0]);
            }
            Instruction::GuestDeallocateString => {
                uwriteln!(self.src, "// GuestDeallocateString(len={}, ptr={})", operands[1], operands[0]);
            }
            Instruction::GuestDeallocateVariant { blocks } => {
                let blocks = self
                    .blocks
                    .drain(self.blocks.len() - blocks..)
                    .collect::<Vec<_>>();

                uwriteln!(self.src, "// GuestDeallocateVariant(tag={})", operands[0]);

                for (i, (block, results)) in blocks.into_iter().enumerate() {
                    assert!(results.is_empty());
                    uwriteln!(self.src, "// -- GuestDeallocateVariant case: {} ", i);
                    self.src.push_str(&block);
                }
            }
            Instruction::GuestDeallocateList { .. } => {
                let (body, _) = self.blocks.pop().unwrap();
                uwriteln!(self.src, "// GuestDeallocateList(len={}, ptr={})", operands[1], operands[0]);
                uwrite!(self.src, "{body}");
            }

            i => unimplemented!("{:?}", i),
        }
    }

    fn return_pointer(&mut self, size: usize, align: usize) -> String {
        let ptr = self.locals.tmp("ptr");
        uwriteln!(self.src, "val {ptr} = /* RETURN_ADDRESS_ALLOC(size={size}, align={align})*/ allocator.allocate({size}).address.toInt()");
        ptr
    }

    fn push_block(&mut self) {
        let prev = mem::take(&mut self.src);
        self.block_storage.push(prev);
    }

    fn finish_block(&mut self, operands: &mut Vec<String>) {
        let to_restore = self.block_storage.pop().unwrap();
        let src = mem::replace(&mut self.src, to_restore);
        self.blocks.push((src.into(), mem::take(operands)));
    }

    fn sizes(&self) -> &SizeAlign {
        &self.gen.gen.sizes
    }

    fn is_list_canonical(&self, _: &Resolve, _: &Type) -> bool {
        false
    }
}

fn perform_cast(op: &String, cast: &Bitcast) -> String {
    match cast {
        Bitcast::I32ToF32 => format!("Float.fromBits({op})"),
        Bitcast::I64ToF32 => format!("Float.fromBits({op}.toInt())"),
        Bitcast::I64ToF64 => format!("Double.fromBits({op})"),
        Bitcast::F32ToI32
        | Bitcast::F64ToI64 => format!("{op}.toRawBits()"),
        Bitcast::F32ToI64 => format!("{op}.toRawBits().toLong()"),
        Bitcast::I32ToI64 => format!("{op}.toLong()"),
        Bitcast::I64ToI32 => format!("{op}.toInt()"),
        Bitcast::None => op.to_string(),
        Bitcast::I32ToP |
        Bitcast::PToI32 |
        Bitcast::LToP |
        Bitcast::I32ToL |
        Bitcast::LToI32 |
        Bitcast::PToL => format!("{op}"),

        Bitcast::I64ToP64 |
        Bitcast::P64ToI64 => format!("{op}"),

        Bitcast::LToI64 | Bitcast::PToP64 => format!("({op}).toLong()"),
        Bitcast::I64ToL | Bitcast::P64ToP => format!("({op}).toInt()"),

        Bitcast::Sequence(sequence) => {
            let [first, second] = &**sequence;
            perform_cast(&perform_cast(op, first), second)
        }
    }
}


fn wasm_type(ty: WasmType) -> &'static str {
    match ty {
        WasmType::I32 => "Int",
        WasmType::I64 => "Long",
        WasmType::F32 => "Float",
        WasmType::F64 => "Double",
        WasmType::Pointer => "Int",
        WasmType::PointerOrI64 => "Long",
        WasmType::Length => "Int"
    }
}

pub fn is_option_type(resolve: &Resolve, ty: &Type) -> bool {
    match ty {
        Type::Id(id) => match resolve.types[*id].kind {
            TypeDefKind::Option(_) => true,
            _ => false
        }
        _ => false
    }
}

pub fn to_kotlin_ident(name: &str) -> String {
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