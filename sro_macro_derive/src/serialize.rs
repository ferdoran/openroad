use darling::FromAttributes;
use proc_macro2::{Ident, TokenStream};
use proc_macro_error::abort;
use quote::{format_ident, quote, quote_spanned};
use syn::{spanned::Spanned, Data, Field, Fields, Index, Variant};

use crate::{
    get_type_of, get_variant_value, PacketArgs, PacketFieldArgs, UsedType, DEFAULT_LIST_TYPE,
};

pub(crate) fn serialize(ident: &Ident, data: &Data, args: PacketArgs) -> TokenStream {
    match *data {
        Data::Struct(ref data) => match data.fields {
            Fields::Named(ref fields) => {
                let content = fields.named.iter().map(|field| {
                    let ident = field.ident.as_ref().expect("i failed");
                    generate_for_field(field, quote!(self.#ident))
                });
                quote_spanned! { ident.span() =>
                    #(#content)*
                }
            }
            Fields::Unnamed(ref fields) => {
                let content = fields.unnamed.iter().enumerate().map(|(i, field)| {
                    let index = Index::from(i);
                    generate_for_field(field, quote!(self.#index))
                });
                quote_spanned! { ident.span() =>
                    #(#content)*
                }
            }
            Fields::Unit => {
                quote!()
            }
        },
        Data::Enum(ref data) => {
            let size = args.size.unwrap_or(1);
            let variant_content = data
                .variants
                .iter()
                .map(|variant| generate_for_variant(ident, variant, size));
            quote_spanned! { ident.span() =>
                match &self {
                    #(#variant_content),*
                }
            }
        }
        _ => abort!(ident, "Only structs and enums are supported yet"),
    }
}

fn generate_for_field(field: &Field, ident: TokenStream) -> TokenStream {
    let typ = get_type_of(&field.ty);
    let args = PacketFieldArgs::from_attributes(&field.attrs).expect("i failed");
    match typ {
        UsedType::Primitive => {
            quote_spanned! {field.span() =>
                #ident.serialize_to(buf);
            }
        }
        UsedType::String => {
            // The u16 length prefix counts wire units, not Rust bytes: UTF-8
            // strings use the byte count, but UTF-16 (`size = 2`) strings must
            // count code units — `len()` would be wrong for any non-ASCII text.
            match args.size.unwrap_or(1) {
                1 => quote_spanned! {field.span() =>
                    (#ident.len() as u16).serialize_to(buf);
                    for byte in #ident.as_bytes() {
                        byte.serialize_to(buf);
                    }
                },
                2 => quote_spanned! {field.span() =>
                    (#ident.encode_utf16().count() as u16).serialize_to(buf);
                    for utf_char in #ident.encode_utf16() {
                        utf_char.serialize_to(buf);
                    }
                },
                _ => abort!(field, "Unknown String len"),
            }
        }
        UsedType::Array(_) => {
            quote_spanned! {field.span() =>
                for inner in #ident {
                    inner.serialize_to(buf);
                }
            }
        }
        UsedType::Collection(inner) => {
            let length_type = args.list_type.as_deref().unwrap_or(DEFAULT_LIST_TYPE);
            // TODO: this does not handle double length strings.
            let inner_ty = match get_type_of(inner) {
                UsedType::Primitive => quote!(inner.serialize_to(buf)),
                UsedType::String => quote! {
                    (inner.len() as u16).serialize_to(buf);
                    for byte in inner.as_bytes() {
                        byte.serialize_to(buf);
                    }
                },
                _ => abort!(field, "Cannot nest collection-like types"),
            };

            let size = args.size.unwrap_or(1);
            if length_type == "break" {
                let continue_lit = get_variant_value(&ident, 1, size);
                let break_lit = get_variant_value(&ident, 2, size);
                quote_spanned! {field.span() =>
                    for inner in #ident.iter() {
                        #continue_lit.serialize_to(buf);
                        #inner_ty;
                    }
                    #break_lit.serialize_to(buf);
                }
            } else if length_type == "has-more" {
                let continue_lit = get_variant_value(&ident, 1, size);
                let break_lit = get_variant_value(&ident, 0, size);
                quote_spanned! {field.span() =>
                    for inner in #ident.iter() {
                        #continue_lit.serialize_to(buf);
                        #inner_ty;
                    }
                    #break_lit.serialize_to(buf);
                }
            } else if length_type == "length" {
                let size_type = match size {
                    1 => quote!(u8),
                    2 => quote!(u16),
                    3 => quote!(u32),
                    4 => quote!(u64),
                    _ => abort!(ident, "Could not determine size for list."),
                };
                quote_spanned! {field.span() =>
                    (#ident.len() as #size_type).serialize_to(buf);
                    for inner in #ident.iter() {
                        #inner_ty;
                    }
                }
            } else {
                quote_spanned! {field.span() =>
                    for inner in #ident.iter() {
                        #inner_ty;
                    }
                }
            }
        }
        UsedType::Option(inner) => {
            let inner_type = match get_type_of(inner) {
                UsedType::Primitive => quote!(inner.serialize_to(buf)),
                UsedType::String => quote! {
                    (inner.len() as u16).serialize_to(buf);
                    for byte in inner.as_bytes() {
                        byte.serialize_to(buf);
                    }
                },
                _ => abort!(field, "only primitives or strings are supported"),
            };
            // `when`-conditional options carry no presence flag on the wire:
            // the condition over previously written fields already encodes
            // whether the value follows (mirrors deserialize.rs), so Some/None
            // alone decides what is emitted. Plain options keep the 1u8/0u8
            // flag byte.
            if args.when.is_some() {
                quote_spanned! {field.span() =>
                    if let Some(inner) = &#ident {
                        #inner_type;
                    }
                }
            } else {
                quote_spanned! {field.span() =>
                    match &#ident {
                        Some(inner) => {
                            1u8.serialize_to(buf);
                            #inner_type;
                        },
                        None => 0u8.serialize_to(buf),
                    }
                }
            }
        }
        UsedType::Tuple(items) => {
            let def = (0..items.len())
                .map(|index| format_ident!("t{}", index))
                .collect::<Vec<Ident>>();

            quote_spanned! {field.span() =>
                let (#(#def),*) = &#ident;
                #(#def.serialize_to(buf);)*
            }
        }
    }
}

fn generate_for_variant(ident: &Ident, variant: &Variant, size: usize) -> TokenStream {
    let attributes = PacketFieldArgs::from_attributes(&variant.attrs).expect("i failed");
    let variant_name = &variant.ident;
    let value_output = if size > 0 {
        let value = attributes
            .value
            .expect("When size is not zero, value should be set.");
        let value = get_variant_value(variant_name, value, size);
        quote_spanned! { variant_name.span() =>
            #value.serialize_to(buf);
        }
    } else {
        quote!()
    };
    match &variant.fields {
        Fields::Named(fields) => {
            let idents = fields
                .named
                .iter()
                .map(|field| field.ident.as_ref().expect("i failed"))
                .collect::<Vec<&Ident>>();

            let content = fields
                .named
                .iter()
                .zip(&idents)
                .map(|(field, ident)| generate_for_field(field, quote!(#ident)));

            quote_spanned! {variant_name.span()=>
                #ident::#variant_name { #(#idents),* } => {
                    #value_output
                    #(#content)*
                }
            }
        }
        Fields::Unnamed(fields) => {
            let idents = (0..fields.unnamed.len())
                .map(|i| format_ident!("t{}", i))
                .collect::<Vec<Ident>>();
            let content = fields
                .unnamed
                .iter()
                .zip(&idents)
                .map(|(field, ident)| generate_for_field(field, quote!(#ident)));

            quote_spanned! {variant_name.span()=>
                #ident::#variant_name(#(#idents),*) => {
                    #value_output
                    #(#content)*
                }
            }
        }
        Fields::Unit => {
            quote_spanned! {variant_name.span()=>
                #ident::#variant_name => {
                    #value_output
                }
            }
        }
    }
}
