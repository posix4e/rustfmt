// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::fmt;

use syntax::ast;
use syntax::print::pprust;
use syntax::codemap::{self, Span, BytePos, CodeMap};

use lists::{itemize_list, write_list, ListTactic, SeparatorTactic, ListFormatting};
use rewrite::{Rewrite, RewriteContext};
use utils::{extra_offset, span_after};

impl Rewrite for ast::Path {
    fn rewrite(&self, context: &RewriteContext, width: usize, offset: usize) -> Option<String> {
        rewrite_path(context, None, self, width, offset)
    }
}

// Does not wrap on simple segments.
pub fn rewrite_path(context: &RewriteContext,
                    qself: Option<&ast::QSelf>,
                    path: &ast::Path,
                    width: usize,
                    offset: usize)
                    -> Option<String> {
    let skip_count = qself.map(|x| x.position).unwrap_or(0);

    let mut result = if path.global {
        "::".to_owned()
    } else {
        String::new()
    };

    let mut span_lo = path.span.lo;

    if let Some(ref qself) = qself {
        result.push('<');
        result.push_str(&pprust::ty_to_string(&qself.ty));
        result.push_str(" as ");

        let extra_offset = extra_offset(&result, offset);
        // 3 = ">::".len()
        let budget = try_opt!(width.checked_sub(extra_offset)) - 3;

        result = try_opt!(rewrite_path_segments(result,
                                                path.segments.iter().take(skip_count),
                                                span_lo,
                                                path.span.hi,
                                                context,
                                                budget,
                                                offset + extra_offset));

        result.push_str(">::");
        span_lo = qself.ty.span.hi + BytePos(1);
    }

    let extra_offset = extra_offset(&result, offset);
    let budget = try_opt!(width.checked_sub(extra_offset));
    rewrite_path_segments(result,
                          path.segments.iter().skip(skip_count),
                          span_lo,
                          path.span.hi,
                          context,
                          budget,
                          offset + extra_offset)
}

fn rewrite_path_segments<'a, I>(mut buffer: String,
                                iter: I,
                                mut span_lo: BytePos,
                                span_hi: BytePos,
                                context: &RewriteContext,
                                width: usize,
                                offset: usize)
                                -> Option<String>
    where I: Iterator<Item = &'a ast::PathSegment>
{
    let mut first = true;

    for segment in iter {
        let extra_offset = extra_offset(&buffer, offset);
        let remaining_width = try_opt!(width.checked_sub(extra_offset));
        let new_offset = offset + extra_offset;
        let segment_string = try_opt!(rewrite_segment(segment,
                                                      &mut span_lo,
                                                      span_hi,
                                                      context,
                                                      remaining_width,
                                                      new_offset));

        if first {
            first = false;
        } else {
            buffer.push_str("::");
        }

        buffer.push_str(&segment_string);
    }

    Some(buffer)
}

enum SegmentParam<'a> {
    LifeTime(&'a ast::Lifetime),
    Type(&'a ast::Ty),
    Binding(&'a ast::TypeBinding),
}

impl<'a> SegmentParam<'a> {
    fn get_span(&self) -> Span {
        match *self {
            SegmentParam::LifeTime(ref lt) => lt.span,
            SegmentParam::Type(ref ty) => ty.span,
            SegmentParam::Binding(ref binding) => binding.span,
        }
    }
}

impl<'a> fmt::Display for SegmentParam<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            SegmentParam::LifeTime(ref lt) => {
                write!(f, "{}", pprust::lifetime_to_string(lt))
            }
            SegmentParam::Type(ref ty) => {
                write!(f, "{}", pprust::ty_to_string(ty))
            }
            SegmentParam::Binding(ref binding) => {
                write!(f, "{} = {}", binding.ident, pprust::ty_to_string(&*binding.ty))
            }
        }
    }
}

// This is a dirty hack to determine if we're in an expression or not. Generic
// parameters are passed differently in expressions and items. We'd declare
// a struct with Foo<A, B>, but call its functions with Foo::<A, B>::f().
// We'd really rather not do this, but there doesn't seem to be an alternative
// at this point.
// FIXME: fails with spans containing comments with the characters < or :
fn get_path_separator(codemap: &CodeMap,
                      path_start: BytePos,
                      segment_start: BytePos)
                      -> &'static str {
    let span = codemap::mk_sp(path_start, segment_start);
    let snippet = codemap.span_to_snippet(span).unwrap();

    for c in snippet.chars().rev() {
        if c == ':' {
            return "::"
        } else if c.is_whitespace() || c == '<' {
            continue;
        } else {
            return "";
        }
    }

    unreachable!();
}

// Formats a path segment. There are some hacks involved to correctly determine
// the segment's associated span since it's not part of the AST.
//
// The span_lo is assumed to be greater than the end of any previous segment's
// parameters and lesser or equal than the start of current segment.
//
// span_hi is assumed equal to the end of the entire path.
//
// When the segment contains a positive number of parameters, we update span_lo
// so that invariants described above will hold for the next segment.
fn rewrite_segment(segment: &ast::PathSegment,
                   span_lo: &mut BytePos,
                   span_hi: BytePos,
                   context: &RewriteContext,
                   width: usize,
                   offset: usize)
                   -> Option<String> {
    let ident_len = segment.identifier.to_string().len();
    let width = try_opt!(width.checked_sub(ident_len));
    let offset = offset + ident_len;

    let params = match segment.parameters {
        ast::PathParameters::AngleBracketedParameters(ref data) if data.lifetimes.len() > 0 ||
                                                                   data.types.len() > 0 ||
                                                                   data.bindings.len() > 0 => {
            let param_list = data.lifetimes.iter()
                                           .map(SegmentParam::LifeTime)
                                           .chain(data.types.iter()
                                                      .map(|x| SegmentParam::Type(&*x)))
                                           .chain(data.bindings.iter()
                                                      .map(|x| SegmentParam::Binding(&*x)))
                                           .collect::<Vec<_>>();

            let next_span_lo = param_list.last().unwrap().get_span().hi + BytePos(1);
            let list_lo = span_after(codemap::mk_sp(*span_lo, span_hi), "<", context.codemap);
            let separator = get_path_separator(context.codemap, *span_lo, list_lo);

            let items = itemize_list(context.codemap,
                                     Vec::new(),
                                     param_list.into_iter(),
                                     ",",
                                     ">",
                                     |param| param.get_span().lo,
                                     |param| param.get_span().hi,
                                     ToString::to_string,
                                     list_lo,
                                     span_hi);

            // 1 for <
            let extra_offset = 1 + separator.len();
            // 1 for >
            let list_width = try_opt!(width.checked_sub(extra_offset + 1));

            let fmt = ListFormatting {
                tactic: ListTactic::HorizontalVertical,
                separator: ",",
                trailing_separator: SeparatorTactic::Never,
                indent: offset + extra_offset,
                h_width: list_width,
                v_width: list_width,
                ends_with_newline: false,
            };

            // update pos
            *span_lo = next_span_lo;

            format!("{}<{}>", separator, write_list(&items, &fmt))
        }
        ast::PathParameters::ParenthesizedParameters(ref data) => {
            let output = match data.output {
                Some(ref ty) => format!(" -> {}", pprust::ty_to_string(&*ty)),
                None => String::new()
            };

            let list_lo = span_after(codemap::mk_sp(*span_lo, span_hi), "(", context.codemap);
            let items = itemize_list(context.codemap,
                                     Vec::new(),
                                     data.inputs.iter(),
                                     ",",
                                     ")",
                                     |ty| ty.span.lo,
                                     |ty| ty.span.hi,
                                     |ty| pprust::ty_to_string(ty),
                                     list_lo,
                                     span_hi);

            // 2 for ()
            let budget = try_opt!(width.checked_sub(output.len() + 2));

            let fmt = ListFormatting {
                tactic: ListTactic::HorizontalVertical,
                separator: ",",
                trailing_separator: SeparatorTactic::Never,
                // 1 for (
                indent: offset + 1,
                h_width: budget,
                v_width: budget,
                ends_with_newline: false,
            };

            // update pos
            *span_lo = data.inputs.last().unwrap().span.hi + BytePos(1);

            format!("({}){}", write_list(&items, &fmt), output)
        }
        _ => String::new()
    };

    Some(format!("{}{}", segment.identifier, params))
}

impl Rewrite for ast::WherePredicate {
    fn rewrite(&self, context: &RewriteContext, width: usize, offset: usize) -> Option<String> {
        // TODO dead spans?
        // TODO assumes we'll always fit on one line...
        Some(match self {
            &ast::WherePredicate::BoundPredicate(ast::WhereBoundPredicate{ref bound_lifetimes,
                                                                          ref bounded_ty,
                                                                          ref bounds,
                                                                          ..}) => {
                if bound_lifetimes.len() > 0 {
                    let lifetime_str = bound_lifetimes.iter().map(|lt| {
                                           lt.rewrite(context, width, offset).unwrap()
                                       }).collect::<Vec<_>>().join(", ");
                    let type_str = pprust::ty_to_string(bounded_ty);
                    // 8 = "for<> : ".len()
                    let used_width = lifetime_str.len() + type_str.len() + 8;
                    let bounds_str = bounds.iter().map(|ty_bound| {
                                         ty_bound.rewrite(context,
                                                          width - used_width,
                                                          offset + used_width)
                                                 .unwrap()
                                     }).collect::<Vec<_>>().join(" + ");

                    format!("for<{}> {}: {}", lifetime_str, type_str, bounds_str)
                } else {
                    let type_str = pprust::ty_to_string(bounded_ty);
                    // 2 = ": ".len()
                    let used_width = type_str.len() + 2;
                    let bounds_str = bounds.iter().map(|ty_bound| {
                                         ty_bound.rewrite(context,
                                                          width - used_width,
                                                          offset + used_width)
                                                 .unwrap()
                                     }).collect::<Vec<_>>().join(" + ");

                    format!("{}: {}", type_str, bounds_str)
                }
            }
            &ast::WherePredicate::RegionPredicate(ast::WhereRegionPredicate{ref lifetime,
                                                                            ref bounds,
                                                                            ..}) => {
                format!("{}: {}",
                        pprust::lifetime_to_string(lifetime),
                        bounds.iter().map(pprust::lifetime_to_string)
                              .collect::<Vec<_>>().join(" + "))
            }
            &ast::WherePredicate::EqPredicate(ast::WhereEqPredicate{ref path, ref ty, ..}) => {
                let ty_str = pprust::ty_to_string(ty);
                // 3 = " = ".len()
                let used_width = 3 + ty_str.len();
                let path_str = try_opt!(path.rewrite(context,
                                                     width - used_width,
                                                     offset + used_width));
                format!("{} = {}", path_str, ty_str)
            }
        })
    }
}

impl Rewrite for ast::LifetimeDef {
    fn rewrite(&self, _: &RewriteContext, _: usize, _: usize) -> Option<String> {
        if self.bounds.len() == 0 {
            Some(pprust::lifetime_to_string(&self.lifetime))
        } else {
            Some(format!("{}: {}",
                         pprust::lifetime_to_string(&self.lifetime),
                         self.bounds.iter().map(pprust::lifetime_to_string)
                                    .collect::<Vec<_>>().join(" + ")))
        }
    }
}

impl Rewrite for ast::TyParamBound {
    fn rewrite(&self, context: &RewriteContext, width: usize, offset: usize) -> Option<String> {
        match *self {
            ast::TyParamBound::TraitTyParamBound(ref tref, ast::TraitBoundModifier::None) => {
                tref.rewrite(context, width, offset)
            }
            ast::TyParamBound::TraitTyParamBound(ref tref, ast::TraitBoundModifier::Maybe) => {
                Some(format!("?{}", try_opt!(tref.rewrite(context, width - 1, offset + 1))))
            }
            ast::TyParamBound::RegionTyParamBound(ref l) => {
                Some(pprust::lifetime_to_string(l))
            }
        }
    }
}

// FIXME: this assumes everything will fit on one line
impl Rewrite for ast::TyParam {
    fn rewrite(&self, context: &RewriteContext, width: usize, offset: usize) -> Option<String> {
        let mut result = String::with_capacity(128);
        result.push_str(&self.ident.to_string());
        if self.bounds.len() > 0 {
            result.push_str(": ");

            let bounds = self.bounds.iter().map(|ty_bound| {
                ty_bound.rewrite(context, width, offset).unwrap()
            }).collect::<Vec<_>>().join(" + ");

            result.push_str(&bounds);
        }
        if let Some(ref def) = self.default {
            result.push_str(" = ");
            result.push_str(&pprust::ty_to_string(&def));
        }

        Some(result)
    }
}

// FIXME: this assumes everything will fit on one line
impl Rewrite for ast::PolyTraitRef {
    fn rewrite(&self, context: &RewriteContext, width: usize, offset: usize) -> Option<String> {
        if self.bound_lifetimes.len() > 0 {
            let lifetime_str = self.bound_lifetimes.iter().map(|lt| {
                lt.rewrite(context, width, offset).unwrap()
            }).collect::<Vec<_>>().join(", ");
            // 6 is "for<> ".len()
            let extra_offset = lifetime_str.len() + 6;
            let max_path_width = try_opt!(width.checked_sub(extra_offset));
            let path_str = try_opt!(self.trait_ref.path.rewrite(context,
                                                                max_path_width,
                                                                offset + extra_offset));

            Some(format!("for<{}> {}", lifetime_str, path_str))
        } else {
            self.trait_ref.path.rewrite(context, width, offset)
        }
    }
}
