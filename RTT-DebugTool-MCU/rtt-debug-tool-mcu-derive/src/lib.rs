use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, Meta, Type};

/// 从类型路径中提取最后一个标识符
fn last_ident(ty: &Type) -> Option<String> {
    if let Type::Path(tp) = ty {
        tp.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
    }
}

/// 从 `#[watch(...)]` 属性中检查是否有指定 key
fn has_watch_attr(attrs: &[syn::Attribute], key: &str) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("watch") {
            return false;
        }
        if let Meta::List(ml) = &attr.meta {
            return ml.tokens.to_string().contains(key);
        }
        false
    })
}

/// 类型名 → WatchValueKind::XXX 和 type_name 字面量
fn primitive_kind(type_name: &str) -> Option<(&str, &str)> {
    match type_name {
        "f32" => Some(("F32", "f32")),
        "f64" => Some(("F64", "f64")),
        "i8" => Some(("I8", "i8")),
        "i16" => Some(("I16", "i16")),
        "i32" => Some(("I32", "i32")),
        "i64" => Some(("I64", "i64")),
        "u8" => Some(("U8", "u8")),
        "u16" => Some(("U16", "u16")),
        "u32" => Some(("U32", "u32")),
        "u64" => Some(("U64", "u64")),
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
            _ => {
                return syn::Error::new_spanned(ds.struct_token, "Watch 仅支持具名字段结构体")
                    .to_compile_error()
                    .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(struct_name, "Watch 仅支持结构体")
                .to_compile_error()
                .into();
        }
    };

    let register_calls: Vec<_> = fields
        .iter()
        .filter_map(|field| {
            let field_name = field.ident.as_ref()?;

            // #[watch(skip)] → 跳过
            if has_watch_attr(&field.attrs, "skip") {
                return None;
            }

            let type_ident_str = last_ident(&field.ty)?;
            let type_ident = format_ident!("{}", type_ident_str);
            let is_readonly = has_watch_attr(&field.attrs, "readonly");

            if let Some((kind_str, type_name_str)) = primitive_kind(&type_ident_str) {
                let kind = format_ident!("{}", kind_str);
                let access = if is_readonly {
                    quote! { ::rtt_debug_tool_mcu::watch_value::Access::ReadOnly }
                } else {
                    quote! { ::rtt_debug_tool_mcu::watch_value::Access::ReadWrite }
                };

                let field_name_str = field_name.to_string();

                Some(quote! {
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
                            read_fn: |p| {
                                let cell = unsafe {
                                    &*(p as *const ::core::cell::RefCell<#struct_name>)
                                };
                                ::core::option::Option::Some(
                                    <#type_ident as ::rtt_debug_tool_mcu::watch_value::WatchValue>::watch_read(
                                        &cell.borrow().#field_name
                                    )
                                )
                            },
                            write_fn: |p, raw| {
                                let cell = unsafe {
                                    &*(p as *const ::core::cell::RefCell<#struct_name>)
                                };
                                if let ::core::option::Option::Some(v) =
                                    <#type_ident as ::rtt_debug_tool_mcu::watch_value::WatchValue>::watch_write(raw)
                                {
                                    cell.borrow_mut().#field_name = v;
                                    true
                                } else {
                                    false
                                }
                            },
                        });
                    }
                })
            } else {
                let field_name_str = field_name.to_string();
                let type_label: &str = &type_ident_str;

                Some(quote! {
                    {
                        let _path = ::rtt_debug_tool_mcu::watch_table::path_from_parts(
                            &[parent, #field_name_str]
                        );
                        let _parent = ::rtt_debug_tool_mcu::watch_table::str_to_string64(parent);
                        cb(::rtt_debug_tool_mcu::watch_table::WatchEntry {
                            path: _path,
                            parent: _parent,
                            type_name: #type_label,
                            kind: ::rtt_debug_tool_mcu::watch_value::WatchValueKind::Str(32),
                            access: ::rtt_debug_tool_mcu::watch_value::Access::ReadOnly,
                            ptr,
                            read_fn: |p| {
                                let mut _s: ::heapless::String<32> = ::heapless::String::new();
                                let _ = _s.push_str(#type_label);
                                ::core::option::Option::Some(_s)
                            },
                            write_fn: |_p, _raw| false,
                        });
                    }
                })
            }
        })
        .collect();

    let expanded = quote! {
        impl ::rtt_debug_tool_mcu::watch_table::WatchFields for #struct_name {
            fn walk_fields(
                parent: &'static str,
                ptr: *const (),
                cb: &mut dyn ::core::ops::FnMut(
                    ::rtt_debug_tool_mcu::watch_table::WatchEntry
                ),
            ) {
                #(#register_calls)*
            }
        }
    };

    TokenStream::from(expanded)
}
