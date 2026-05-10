use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, Meta, Type};

fn last_ident(ty: &Type) -> Option<String> {
    if let Type::Path(tp) = ty {
        tp.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
    }
}

fn has_watch_attr(attrs: &[syn::Attribute], key: &str) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("watch") { return false; }
        if let Meta::List(ml) = &attr.meta {
            return ml.tokens.to_string().contains(key);
        }
        false
    })
}

fn primitive_kind(type_name: &str) -> Option<(&str, &str)> {
    match type_name {
        "f32" => Some(("F32", "f32")), "f64" => Some(("F64", "f64")),
        "i8" => Some(("I8", "i8")), "i16" => Some(("I16", "i16")),
        "i32" => Some(("I32", "i32")), "i64" => Some(("I64", "i64")),
        "u8" => Some(("U8", "u8")), "u16" => Some(("U16", "u16")),
        "u32" => Some(("U32", "u32")), "u64" => Some(("U64", "u64")),
        "bool" => Some(("Bool", "bool")),
        _ => None,
    }
}

#[proc_macro_derive(Watch, attributes(watch))]
pub fn derive_watch(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let struct_name = &input.ident;

    let fields = match &input.data {
        Data::Struct(ds) => match &ds.fields {
            Fields::Named(nf) => &nf.named,
            _ => return syn::Error::new_spanned(ds.struct_token, "Watch 仅支持具名字段结构体").to_compile_error().into(),
        },
        _ => return syn::Error::new_spanned(struct_name, "Watch 仅支持结构体").to_compile_error().into(),
    };

    // ── 收集有效字段 (排除 #[watch(skip)]) ──
    struct FieldInfo {
        name: syn::Ident,
        name_str: String,
        type_ident: syn::Ident,
        type_str: String,
        is_primitive: bool,
        is_readonly: bool,
        kind_str: String,
        type_name_str: String,
        index: usize,
    }

    let mut infos: Vec<FieldInfo> = Vec::new();
    for (idx, field) in fields.iter().enumerate() {
        let field_name = match &field.ident { Some(n) => n.clone(), None => continue };
        if has_watch_attr(&field.attrs, "skip") { continue; }
        let type_str = match last_ident(&field.ty) { Some(t) => t, None => continue };
        let is_readonly = has_watch_attr(&field.attrs, "readonly");

        let type_ident = format_ident!("{}", type_str);
        let pk = primitive_kind(&type_str).map(|(k, tn)| (k.to_string(), tn.to_string()));
        if let Some((k, tn)) = pk {
            infos.push(FieldInfo {
                name: field_name.clone(),
                name_str: field_name.to_string(),
                type_ident,
                type_str,
                is_primitive: true,
                is_readonly,
                kind_str: k,
                type_name_str: tn,
                index: infos.len(),
            });
        } else {
            let type_name_copy = type_str.clone();
            infos.push(FieldInfo {
                name: field_name.clone(),
                name_str: field_name.to_string(),
                type_ident,
                type_str,
                is_primitive: false,
                is_readonly: true,
                kind_str: String::new(),
                type_name_str: type_name_copy,
                index: infos.len(),
            });
        }
    }

    // ── 生成 walk_fields 内的注册代码 ──
    let walk_calls: Vec<_> = infos.iter().map(|fi| {
        let field_name = &fi.name;
        let field_name_str = &fi.name_str;
        let idx = fi.index as u16;

        if fi.is_primitive {
            // 基础类型: 直接注册
            let kind = format_ident!("{}", fi.kind_str);
            let access = if fi.is_readonly {
                quote! { ::rtt_debug_tool_mcu::watch_value::Access::ReadOnly }
            } else {
                quote! { ::rtt_debug_tool_mcu::watch_value::Access::ReadWrite }
            };
            let type_ident = &fi.type_ident;
            let type_name_str = &fi.type_name_str;

            quote! {
                {
                    let _path = ::rtt_debug_tool_mcu::watch_table::path_from_parts(
                        &[parent, #field_name_str]
                    );
                    let _parent = ::rtt_debug_tool_mcu::watch_table::str_to_string64(parent);
                    cb(::rtt_debug_tool_mcu::watch_table::WatchEntry {
                        path: _path,
                        parent: _parent,
                        type_name: #type_name_str,
                        kind: ::rtt_debug_tool_mcu::watch_value::WatchValueKind::#kind,
                        access: #access,
                        ptr,
                        field_idx: #idx,
                        read_fn: (|p: *const (), _i: u16|
                            -> ::core::option::Option<::heapless::String<32>>
                        {
                            let cell = unsafe { &*(p as *const ::core::cell::RefCell<#struct_name>) };
                            ::core::option::Option::Some(
                                <#type_ident as ::rtt_debug_tool_mcu::watch_value::WatchValue>::watch_read(
                                    &cell.borrow().#field_name
                                )
                            )
                        }) as fn(*const (), u16) -> ::core::option::Option<::heapless::String<32>>,
                        write_fn: (|p: *const (), _i: u16, raw: &str| -> bool {
                            let cell = unsafe { &*(p as *const ::core::cell::RefCell<#struct_name>) };
                            if let ::core::option::Option::Some(v) =
                                <#type_ident as ::rtt_debug_tool_mcu::watch_value::WatchValue>::watch_write(raw)
                            { cell.borrow_mut().#field_name = v; true }
                            else { false }
                        }) as fn(*const (), u16, &str) -> bool,
                    });
                }
            }
        } else {
            // 复合类型 (嵌套结构体) → 默认平铺子字段
            let inner_ty = &fi.type_ident;
            quote! {
                {
                    let sub_parent = {
                        let mut _s: ::heapless::String<64> = ::heapless::String::new();
                        let _ = _s.push_str(parent);
                        let _ = _s.push('.');
                        let _ = _s.push_str(#field_name_str);
                        _s
                    };
                    for meta in <#inner_ty as ::rtt_debug_tool_mcu::watch_table::WatchFields>::field_meta() {
                        let sub_path = {
                            let mut _s: ::heapless::String<64> = ::heapless::String::new();
                            let _ = _s.push_str(&sub_parent);
                            let _ = _s.push('.');
                            let _ = _s.push_str(meta.name);
                            _s
                        };
                        cb(::rtt_debug_tool_mcu::watch_table::WatchEntry {
                            path: sub_path,
                            parent: sub_parent.clone(),
                            type_name: meta.type_name,
                            kind: meta.kind,
                            access: meta.access,
                            ptr,
                            field_idx: meta.index,
                            read_fn: (|p: *const (), idx: u16|
                                -> ::core::option::Option<::heapless::String<32>>
                            {
                                let cell = unsafe { &*(p as *const ::core::cell::RefCell<#struct_name>) };
                                let borrowed = cell.borrow();
                                <#inner_ty as ::rtt_debug_tool_mcu::watch_table::WatchFields>::dispatch_read(idx, &borrowed.#field_name)
                            }) as fn(*const (), u16) -> ::core::option::Option<::heapless::String<32>>,
                            write_fn: (|p: *const (), idx: u16, raw: &str| -> bool {
                                let cell = unsafe { &*(p as *const ::core::cell::RefCell<#struct_name>) };
                                let mut borrowed = cell.borrow_mut();
                                <#inner_ty as ::rtt_debug_tool_mcu::watch_table::WatchFields>::dispatch_write(idx, &mut borrowed.#field_name, raw)
                            }) as fn(*const (), u16, &str) -> bool,
                        });
                    }
                }
            }
        }
    }).collect();

    // ── 生成 field_meta ──
    let meta_entries: Vec<_> = infos.iter().map(|fi| {
        let name_str = &fi.name_str;
        let idx = fi.index as u16;
        let access = if fi.is_readonly { quote! { ::rtt_debug_tool_mcu::watch_value::Access::ReadOnly } }
                     else { quote! { ::rtt_debug_tool_mcu::watch_value::Access::ReadWrite } };

        if fi.is_primitive {
            let kind = format_ident!("{}", fi.kind_str);
            let tn = &fi.type_name_str;
            quote! {
                ::rtt_debug_tool_mcu::watch_table::WatchFieldMeta {
                    index: #idx, name: #name_str, type_name: #tn,
                    kind: ::rtt_debug_tool_mcu::watch_value::WatchValueKind::#kind,
                    access: #access,
                }
            }
        } else {
            let tn = &fi.type_str;
            quote! {
                ::rtt_debug_tool_mcu::watch_table::WatchFieldMeta {
                    index: #idx, name: #name_str, type_name: #tn,
                    kind: ::rtt_debug_tool_mcu::watch_value::WatchValueKind::Str(32),
                    access: #access,
                }
            }
        }
    }).collect();

    // ── 生成 dispatch_read / dispatch_write ──
    let dispatch_read_arms: Vec<_> = infos.iter().filter(|fi| fi.is_primitive).map(|fi| {
        let idx = fi.index as u16;
        let field_name = &fi.name;
        let type_ident = &fi.type_ident;
        quote! {
            #idx => ::core::option::Option::Some(
                <#type_ident as ::rtt_debug_tool_mcu::watch_value::WatchValue>::watch_read(&this.#field_name)
            ),
        }
    }).collect();

    let dispatch_write_arms: Vec<_> = infos.iter().filter(|fi| fi.is_primitive && !fi.is_readonly).map(|fi| {
        let idx = fi.index as u16;
        let field_name = &fi.name;
        let type_ident = &fi.type_ident;
        quote! {
            #idx => {
                if let ::core::option::Option::Some(v) =
                    <#type_ident as ::rtt_debug_tool_mcu::watch_value::WatchValue>::watch_write(raw)
                { this.#field_name = v; true } else { false }
            },
        }
    }).collect();

    // ── 组装输出 ──
    let expanded = quote! {
        impl ::rtt_debug_tool_mcu::watch_table::WatchFields for #struct_name {
            fn field_meta() -> &'static [::rtt_debug_tool_mcu::watch_table::WatchFieldMeta] {
                &[ #(#meta_entries),* ]
            }

            fn dispatch_read(field_idx: u16, this: &Self) -> ::core::option::Option<::heapless::String<32>> {
                match field_idx {
                    #(#dispatch_read_arms)*
                    _ => ::core::option::Option::None,
                }
            }

            fn dispatch_write(field_idx: u16, this: &mut Self, raw: &str) -> bool {
                match field_idx {
                    #(#dispatch_write_arms)*
                    _ => false,
                }
            }

            fn walk_fields(
                parent: &'static str,
                ptr: *const (),
                cb: &mut dyn ::core::ops::FnMut(
                    ::rtt_debug_tool_mcu::watch_table::WatchEntry
                ),
            ) {
                #(#walk_calls)*
            }
        }
    };

    TokenStream::from(expanded)
}
