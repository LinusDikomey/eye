use std::iter::Peekable;

use dmap::DHashMap;

use crate::{
    Argument, BlockId, Environment, FunctionIr, Primitive, Ref, Type, TypeId, TypeIds, Types,
    builder::Builder,
};

pub fn parse_function_body(env: &Environment, s: &str) -> (FunctionIr, Types) {
    let mut ir = Builder::new(env);
    let unit = ir.types.add(Type::Tuple(TypeIds::EMPTY));
    let mut refs = dmap::new();
    let mut blocks = dmap::new();
    for (line_number, line) in s.lines().enumerate() {
        let line_number = line_number + 1;
        let unexpected = |tok: Token| panic!("unexpected token {tok:?} in line {line_number}");
        let mut tokens = tokenize(line).peekable();
        match tokens.next() {
            None => {}
            Some(Token::Ident(s)) => match tokens.next().expect("expected something after ident") {
                Token::Colon => {
                    assert!(tokens.next().is_none());
                    let id = *blocks.entry(s).or_insert_with(|| ir.create_block());
                    ir.begin_block(id, []);
                    blocks.insert(s, id);
                }
                Token::LParen => todo!("block arg parsing"),
                Token::Dot => {
                    let Some(Token::Ident(func)) = tokens.next() else {
                        panic!("expected function after dot");
                    };
                    let (args, ty) = parse_args(tokens, &mut ir, &refs, &mut blocks, unit);
                    let module = env.modules_by_name[s];
                    let id = env[module]
                        .functions
                        .iter()
                        .position(|f| &*f.name == func)
                        .unwrap_or_else(|| panic!("function '{s}.{func}' not found"))
                        as u32;
                    let id = crate::FunctionId {
                        module,
                        function: crate::LocalFunctionId(id),
                    };
                    ir.append((id, args, ty));
                }
                tok => unexpected(tok),
            },
            Some(Token::Reg(r_name)) => {
                let Some(Token::Equals) = tokens.next() else {
                    panic!("expected equals after reg on line {line_number}");
                };
                let (Some(Token::Ident(module_name)), Some(Token::Dot), Some(Token::Ident(func))) =
                    (tokens.next(), tokens.next(), tokens.next())
                else {
                    panic!("expected module.function after '=' on line {line_number}")
                };
                let (args, ty) = parse_args(tokens, &mut ir, &refs, &mut blocks, unit);
                let module = *env
                    .modules_by_name
                    .get(module_name)
                    .unwrap_or_else(|| panic!("unknown dialect {module_name}"));
                let id = env[module]
                    .functions
                    .iter()
                    .position(|f| &*f.name == func)
                    .unwrap_or_else(|| panic!("function '{module_name}.{func}' not found"))
                    as u32;
                let id = crate::FunctionId {
                    module,
                    function: crate::LocalFunctionId(id),
                };
                let r = ir.append((id, args, ty));
                let prev = refs.insert(r_name, r);
                if prev.is_some() {
                    panic!("duplicate ref '{r_name} in line {line_number}'");
                }
            }
            Some(tok) => unexpected(tok),
        }
    }
    ir.finish_body()
}

fn parse_args<'s>(
    mut tokens: Peekable<impl Iterator<Item = Token<'s>>>,
    ir: &mut Builder<&Environment>,
    refs: &DHashMap<&'s str, Ref>,
    blocks: &mut DHashMap<&'s str, BlockId>,
    unit: TypeId,
) -> (Vec<Argument<'static>>, TypeId) {
    let mut args = Vec::new();
    let mut ty = unit;
    while let Some(tok) = tokens.next() {
        let arg = match tok {
            Token::IntLit(int) => Argument::Int(int),
            Token::Reg(r) => Argument::Ref(
                *refs
                    .get(r)
                    .unwrap_or_else(|| panic!("couldn't find ref %{r}")),
            ),
            Token::Ident("unit") => Argument::Ref(Ref::UNIT),
            Token::Ident("true") => Argument::Ref(Ref::TRUE),
            Token::Ident("false") => Argument::Ref(Ref::FALSE),
            Token::Ident(s) => {
                if let Ok(p) = s.parse::<Primitive>() {
                    Argument::TypeId(ir.types.add(Type::Primitive(p.into())))
                } else {
                    let id = *blocks.entry(s).or_insert_with(|| ir.create_block());
                    Argument::BlockTarget(crate::BlockTarget::new(id))
                }
            }
            Token::ColonColon => {
                let parsed_ty = parse_ty(&mut tokens, &mut ir.types);
                let id = ir.types.add(parsed_ty);
                ty = id;
                break;
            }
            tok => panic!("unexpected arg token {tok:?}"),
        };
        args.push(arg);
    }
    assert!(tokens.next().is_none(), "leftover tokens in line");
    (args, ty)
}

fn parse_ty<'a>(tokens: &mut Peekable<impl Iterator<Item = Token<'a>>>, types: &mut Types) -> Type {
    match tokens.next().expect("type expected") {
        Token::Ident(s) => Type::Primitive(s.parse::<Primitive>().expect("unknown type").into()),
        Token::LParen => {
            let mut fields = Vec::new();
            match tokens.peek().unwrap() {
                Token::RParen => {}
                _ => loop {
                    fields.push(parse_ty(tokens, types));
                    match tokens.next().unwrap() {
                        Token::RParen => break,
                        Token::Comma => {}
                        tok => panic!("unexpected token in tuple: {tok:?}"),
                    }
                },
            }
            Type::Tuple(types.add_multiple(fields))
        }
        tok => panic!("unexpected token for type: {tok:?}"),
    }
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_')
}

fn tokenize(s: &str) -> impl Iterator<Item = Token<'_>> {
    let mut chars = s.char_indices().peekable();
    std::iter::from_fn(move || {
        Some(loop {
            let (i, c) = chars.next()?;
            match c {
                '%' => {
                    let (start, c) = chars.next().unwrap();
                    assert!(ident_char(c));
                    while chars.next_if(|&(_, c)| ident_char(c)).is_some() {}
                    let end = chars.peek().map_or(s.len(), |&(i, _)| i);
                    return Some(Token::Reg(&s[start..end]));
                }
                ':' => {
                    break if chars.next_if(|&(_, c)| c == ':').is_some() {
                        Token::ColonColon
                    } else {
                        break Token::Colon;
                    };
                }
                '=' => break Token::Equals,
                '.' => break Token::Dot,
                '(' => break Token::LParen,
                ')' => break Token::RParen,
                ',' => break Token::Comma,
                '0'..='9' => {
                    let mut x = (c as u8 - b'0') as u64;
                    while let Some((_, c)) = chars.next_if(|(_, c)| c.is_numeric()) {
                        x = 10 * x + (c as u8 - b'0') as u64;
                    }
                    break Token::IntLit(x);
                }
                c if c.is_alphabetic() || c == '_' => {
                    while chars.next_if(|&(_, c)| ident_char(c)).is_some() {}
                    let end = chars.peek().map_or(s.len(), |&(i, _)| i);
                    break Token::Ident(&s[i..end]);
                }
                c if c.is_whitespace() => {}
                _ => panic!("invalid character: {c}"),
            }
        })
    })
}

#[derive(Debug)]
enum Token<'a> {
    IntLit(u64),
    Reg(&'a str),
    Ident(&'a str),
    Equals,
    Colon,
    ColonColon,
    Dot,
    LParen,
    RParen,
    Comma,
}
