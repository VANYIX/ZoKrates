use bimap::BiMap;
use typed_absy::{folder::*, *};
use types::FunctionKey;
use zokrates_field::field::Field;

#[derive(Debug)]
pub struct Inliner<T: Field> {
    modules: TypedModules<T>,
    module_id: TypedModuleId,
    statement_buffer: Vec<TypedStatement<T>>,
    bijection: BiMap<Variable, Variable>,
    next_var_idx: usize,
}

impl<T: Field> Inliner<T> {
    fn with_modules_and_module_id_and_bijection_and_next_var_idx<S: Into<TypedModuleId>>(
        modules: TypedModules<T>,
        module_id: S,
        bijection: BiMap<Variable, Variable>,
        next_var_idx: usize,
    ) -> Self {
        Inliner {
            modules,
            module_id: module_id.into(),
            statement_buffer: vec![],
            bijection,
            next_var_idx,
        }
    }

    pub fn inline(p: TypedProgram<T>) -> TypedProgram<T> {
        let main_module_id = p.main;

        let main_module = p.modules.get(&main_module_id).unwrap().clone();

        let (main_key, main) = main_module
            .functions
            .into_iter()
            .find(|(k, _)| k.id == "main")
            .unwrap();

        let mut inliner = Inliner::with_modules_and_module_id_and_bijection_and_next_var_idx(
            p.modules,
            main_module_id,
            BiMap::new(),
            0,
        );

        let main = inliner.fold_function_symbol(main);

        let split = crate::types::conversions::split();
        let split_key = FunctionKey::with_id("split").signature(split.signature.clone());

        let sha_round = crate::standard::sha_round();
        let sha_round_key = FunctionKey::with_id("sha256").signature(sha_round.signature.clone());

        TypedProgram {
            main: String::from("main"),
            modules: vec![(
                String::from("main"),
                TypedModule {
                    functions: vec![
                        (split_key, TypedFunctionSymbol::Flat(split)),
                        (sha_round_key, TypedFunctionSymbol::Flat(sha_round)),
                        (main_key.clone(), main),
                    ]
                    .into_iter()
                    .collect(),
                    imports: vec![],
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    // try to inline a call to function with key `key` in the context of `self`
    // if inlining succeeds, return the expressions returned by the function call
    // if inlining fails (as in the case of flat function symbols), return the arguments to the function call for further processing
    fn try_inline_call(
        &mut self,
        key: &FunctionKey,
        expressions: Vec<TypedExpression<T>>,
    ) -> Result<Vec<TypedExpression<T>>, Vec<TypedExpression<T>>> {
        // here we clone a function symbol, which is cheap except when it contains the function body, in which case we'd clone anyways
        match self.module().functions.get(&key).unwrap().clone() {
            // if the function called is in the same module, we can go ahead and inline in this module
            TypedFunctionSymbol::Here(function) => {
                // add definitions for the inputs
                let inputs_bindings: Vec<_> = function
                    .arguments
                    .iter()
                    .zip(expressions)
                    .map(|(a, e)| {
                        TypedStatement::Definition(
                            self.fold_assignee(TypedAssignee::Identifier(a.id.clone())),
                            e,
                        )
                    })
                    .collect();

                self.statement_buffer.extend(inputs_bindings);

                // filter out the return statement and keep it aside
                let (statements, mut ret): (Vec<_>, Vec<_>) = function
                    .statements
                    .into_iter()
                    .flat_map(|s| self.fold_statement(s))
                    .partition(|s| match s {
                        TypedStatement::Return(..) => false,
                        _ => true,
                    });

                // add all statements to the buffer
                self.statement_buffer.extend(statements);

                match ret.pop().unwrap() {
                    TypedStatement::Return(exprs) => Ok(exprs),
                    _ => panic!(""),
                }
            }
            // if the function called is in some other module, we switch context to that module and call the function locally there
            TypedFunctionSymbol::There(function_key, module_id) => {
                let current_module = self.change_module(module_id);
                let res = self.try_inline_call(&function_key, expressions)?;
                self.change_module(current_module);
                Ok(res)
            }
            // if the function is a flat symbol, there's nothing we can inline at this stage so we return the inputs
            TypedFunctionSymbol::Flat(_) => Err(expressions),
        }
    }

    // Focus the Inliner on another module with id `module_id` and return the current `module_id`
    fn change_module(&mut self, module_id: TypedModuleId) -> TypedModuleId {
        std::mem::replace(&mut self.module_id, module_id)
    }

    fn module(&self) -> &TypedModule<T> {
        self.modules.get(&self.module_id).unwrap()
    }

    /// Checks if the given name is a not used variable and returns a fresh variable.
    /// # Arguments
    ///
    /// * `variable` - a Variable that holds the name of the variable
    fn use_variable(&mut self, variable: &Variable) -> Variable {
        let name = self.issue_new_name();
        // issue the variable we'll use
        let var = Variable::new(name.clone(), variable.get_type());

        self.bijection.insert(variable.clone(), var.clone());
        var
    }

    fn issue_new_name(&mut self) -> Identifier {
        let var = format!("_{}", self.next_var_idx.to_string());
        self.next_var_idx += 1;
        var
    }
}

impl<T: Field> Folder<T> for Inliner<T> {
    fn fold_parameter(&mut self, p: Parameter) -> Parameter {
        Parameter {
            id: self.use_variable(&p.id),
            ..p
        }
    }

    fn fold_statement(&mut self, s: TypedStatement<T>) -> Vec<TypedStatement<T>> {
        let folded = match s {
            TypedStatement::MultipleDefinition(variables, elist) => match elist {
                TypedExpressionList::FunctionCall(key, expressions, types) => {
                    let expressions: Vec<_> = expressions
                        .into_iter()
                        .map(|e| self.fold_expression(e))
                        .collect();

                    match self.try_inline_call(&key, expressions) {
                        Ok(ret) => variables
                            .into_iter()
                            .zip(ret.into_iter())
                            .map(|(v, e)| {
                                TypedStatement::Definition(
                                    TypedAssignee::Identifier(self.use_variable(&v)),
                                    e,
                                )
                            })
                            .collect(),
                        Err(expressions) => vec![TypedStatement::MultipleDefinition(
                            variables
                                .into_iter()
                                .map(|a| self.use_variable(&a))
                                .collect(),
                            TypedExpressionList::FunctionCall(key, expressions, types),
                        )],
                    }
                }
            },
            s => fold_statement(self, s),
        };
        self.statement_buffer.drain(..).chain(folded).collect()
    }

    fn fold_field_expression(&mut self, e: FieldElementExpression<T>) -> FieldElementExpression<T> {
        match e {
            FieldElementExpression::Identifier(id) => FieldElementExpression::Identifier(
                self.fold_variable(Variable::field_element(id)).id,
            ),
            FieldElementExpression::FunctionCall(key, expressions) => {
                //inline the arguments
                let expressions: Vec<_> = expressions
                    .into_iter()
                    .map(|e| self.fold_expression(e))
                    .collect();

                match self.try_inline_call(&key, expressions) {
                    Ok(mut ret) => match ret.pop().unwrap() {
                        TypedExpression::FieldElement(e) => e,
                        _ => unreachable!(),
                    },
                    Err(expressions) => FieldElementExpression::FunctionCall(key, expressions),
                }
            }
            e => fold_field_expression(self, e),
        }
    }

    fn fold_field_array_expression(
        &mut self,
        e: FieldElementArrayExpression<T>,
    ) -> FieldElementArrayExpression<T> {
        match e {
            FieldElementArrayExpression::Identifier(size, id) => {
                FieldElementArrayExpression::Identifier(
                    size,
                    self.fold_variable(Variable::field_array(id, size)).id,
                )
            }
            FieldElementArrayExpression::FunctionCall(size, key, expressions) => {
                //inline the arguments
                let expressions: Vec<_> = expressions
                    .into_iter()
                    .map(|e| self.fold_expression(e))
                    .collect();

                match self.try_inline_call(&key, expressions) {
                    Ok(mut ret) => match ret.pop().unwrap() {
                        TypedExpression::FieldElementArray(e) => e,
                        _ => unreachable!(),
                    },
                    Err(expressions) => {
                        FieldElementArrayExpression::FunctionCall(size, key, expressions)
                    }
                }
            }
            e => fold_field_array_expression(self, e),
        }
    }

    // use renamed variables
    fn fold_variable(&mut self, v: Variable) -> Variable {
        self.bijection.get_by_left(&v).unwrap().clone()
    }

    // introduce new variables in assignees
    fn fold_assignee(&mut self, a: TypedAssignee<T>) -> TypedAssignee<T> {
        match a {
            TypedAssignee::Identifier(v) => TypedAssignee::Identifier(self.use_variable(&v)),
            TypedAssignee::ArrayElement(box a, box index) => TypedAssignee::ArrayElement(
                box self.fold_assignee(a),
                box self.fold_field_expression(index),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use types::{FunctionKey, Signature, Type};
    use zokrates_field::field::FieldPrime;

    #[test]
    fn call_other_module_without_variables() {
        // // main
        // from "foo" import foo
        // def main() -> (field):
        //    return foo()
        //
        // // foo
        // def foo() -> (field):
        //    return 42
        //
        //
        // // inlined
        // def main() -> (field):
        //    return 42

        let main = TypedModule {
            functions: vec![
                (
                    FunctionKey::with_id("main")
                        .signature(Signature::new().outputs(vec![Type::FieldElement])),
                    TypedFunctionSymbol::Here(TypedFunction {
                        arguments: vec![],
                        statements: vec![TypedStatement::Return(vec![
                            FieldElementExpression::FunctionCall(
                                FunctionKey::with_id("foo")
                                    .signature(Signature::new().outputs(vec![Type::FieldElement])),
                                vec![],
                            )
                            .into(),
                        ])],
                        signature: Signature::new().outputs(vec![Type::FieldElement]),
                    }),
                ),
                (
                    FunctionKey::with_id("foo")
                        .signature(Signature::new().outputs(vec![Type::FieldElement])),
                    TypedFunctionSymbol::There(
                        FunctionKey::with_id("foo")
                            .signature(Signature::new().outputs(vec![Type::FieldElement])),
                        String::from("foo"),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let foo = TypedModule {
            functions: vec![(
                FunctionKey::with_id("foo")
                    .signature(Signature::new().outputs(vec![Type::FieldElement])),
                TypedFunctionSymbol::Here(TypedFunction {
                    arguments: vec![],
                    statements: vec![TypedStatement::Return(vec![
                        FieldElementExpression::Number(FieldPrime::from(42)).into(),
                    ])],
                    signature: Signature::new().outputs(vec![Type::FieldElement]),
                }),
            )]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let modules: HashMap<_, _> = vec![(String::from("main"), main), (String::from("foo"), foo)]
            .into_iter()
            .collect();

        let program = TypedProgram {
            main: String::from("main"),
            modules,
        };

        let program = Inliner::inline(program);

        assert_eq!(program.modules.len(), 1);
        assert_eq!(
            program
                .modules
                .get(&String::from("main"))
                .unwrap()
                .functions
                .get(
                    &FunctionKey::with_id("main")
                        .signature(Signature::new().outputs(vec![Type::FieldElement]))
                )
                .unwrap(),
            &TypedFunctionSymbol::Here(TypedFunction {
                arguments: vec![],
                statements: vec![TypedStatement::Return(vec![
                    FieldElementExpression::Number(FieldPrime::from(42)).into(),
                ])],
                signature: Signature::new().outputs(vec![Type::FieldElement]),
            })
        );
    }

    #[test]
    fn call_other_module_with_variables() {
        // // main
        // from "foo" import foo
        // def main(field a) -> (field):
        //    return a * foo(a)
        //
        // // foo
        // def foo(field a) -> (field):
        //    return a * a
        //
        //
        // // inlined
        // def main() -> (field):
        //    field a_0 = a
        //    return a * a_0 * a_0

        let main = TypedModule {
            functions: vec![
                (
                    FunctionKey::with_id("main").signature(
                        Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement]),
                    ),
                    TypedFunctionSymbol::Here(TypedFunction {
                        arguments: vec![Parameter::private(Variable::field_element("a"))],
                        statements: vec![TypedStatement::Return(vec![
                            FieldElementExpression::Mult(
                                box FieldElementExpression::Identifier(String::from("a")),
                                box FieldElementExpression::FunctionCall(
                                    FunctionKey::with_id("foo").signature(
                                        Signature::new()
                                            .inputs(vec![Type::FieldElement])
                                            .outputs(vec![Type::FieldElement]),
                                    ),
                                    vec![FieldElementExpression::Identifier(String::from("a"))
                                        .into()],
                                ),
                            )
                            .into(),
                        ])],
                        signature: Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement]),
                    }),
                ),
                (
                    FunctionKey::with_id("foo").signature(
                        Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement]),
                    ),
                    TypedFunctionSymbol::There(
                        FunctionKey::with_id("foo").signature(
                            Signature::new()
                                .inputs(vec![Type::FieldElement])
                                .outputs(vec![Type::FieldElement]),
                        ),
                        String::from("foo"),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let foo = TypedModule {
            functions: vec![(
                FunctionKey::with_id("foo").signature(
                    Signature::new()
                        .inputs(vec![Type::FieldElement])
                        .outputs(vec![Type::FieldElement]),
                ),
                TypedFunctionSymbol::Here(TypedFunction {
                    arguments: vec![Parameter::private(Variable::field_element("a"))],
                    statements: vec![TypedStatement::Return(vec![FieldElementExpression::Mult(
                        box FieldElementExpression::Identifier(String::from("a")),
                        box FieldElementExpression::Identifier(String::from("a")),
                    )
                    .into()])],
                    signature: Signature::new()
                        .inputs(vec![Type::FieldElement])
                        .outputs(vec![Type::FieldElement]),
                }),
            )]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let modules: HashMap<_, _> = vec![(String::from("main"), main), (String::from("foo"), foo)]
            .into_iter()
            .collect();

        let program: TypedProgram<FieldPrime> = TypedProgram {
            main: String::from("main"),
            modules,
        };

        let program = Inliner::inline(program);

        assert_eq!(program.modules.len(), 1);

        assert_eq!(
            program
                .modules
                .get(&String::from("main"))
                .unwrap()
                .functions
                .get(
                    &FunctionKey::with_id("main").signature(
                        Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement])
                    )
                )
                .unwrap(),
            &TypedFunctionSymbol::Here(TypedFunction {
                arguments: vec![Parameter::private(Variable::field_element("_0"))],
                statements: vec![
                    TypedStatement::Definition(
                        TypedAssignee::Identifier(Variable::field_element("_1")),
                        FieldElementExpression::Identifier(String::from("_0")).into()
                    ),
                    TypedStatement::Return(vec![FieldElementExpression::Mult(
                        box FieldElementExpression::Identifier(String::from("_0")),
                        box FieldElementExpression::Mult(
                            box FieldElementExpression::Identifier(String::from("_1")),
                            box FieldElementExpression::Identifier(String::from("_1"))
                        )
                    )
                    .into(),])
                ],
                signature: Signature::new()
                    .inputs(vec![Type::FieldElement])
                    .outputs(vec![Type::FieldElement]),
            })
        );
    }

    #[test]
    fn multi_def_from_other_module() {
        // // foo
        // def foo() -> (field):
        //     return 42

        // // main
        // def main() -> (field):
        //     field b = foo()
        //     return b

        // inlined
        // def main() -> (field)
        //     field _0 = 42
        //     return _0

        let main = TypedModule {
            functions: vec![
                (
                    FunctionKey::with_id("main")
                        .signature(Signature::new().outputs(vec![Type::FieldElement])),
                    TypedFunctionSymbol::Here(TypedFunction {
                        arguments: vec![],
                        statements: vec![
                            TypedStatement::MultipleDefinition(
                                vec![Variable::field_element("a")],
                                TypedExpressionList::FunctionCall(
                                    FunctionKey::with_id("foo").signature(
                                        Signature::new().outputs(vec![Type::FieldElement]),
                                    ),
                                    vec![],
                                    vec![Type::FieldElement],
                                ),
                            ),
                            TypedStatement::Return(vec![FieldElementExpression::Identifier(
                                String::from("a"),
                            )
                            .into()]),
                        ],
                        signature: Signature::new().outputs(vec![Type::FieldElement]),
                    }),
                ),
                (
                    FunctionKey::with_id("foo")
                        .signature(Signature::new().outputs(vec![Type::FieldElement])),
                    TypedFunctionSymbol::There(
                        FunctionKey::with_id("foo")
                            .signature(Signature::new().outputs(vec![Type::FieldElement])),
                        String::from("foo"),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let foo = TypedModule {
            functions: vec![(
                FunctionKey::with_id("foo")
                    .signature(Signature::new().outputs(vec![Type::FieldElement])),
                TypedFunctionSymbol::Here(TypedFunction {
                    arguments: vec![],
                    statements: vec![TypedStatement::Return(vec![
                        FieldElementExpression::Number(FieldPrime::from(42)).into(),
                    ])],
                    signature: Signature::new().outputs(vec![Type::FieldElement]),
                }),
            )]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let modules: HashMap<_, _> = vec![(String::from("main"), main), (String::from("foo"), foo)]
            .into_iter()
            .collect();

        let program = TypedProgram {
            main: String::from("main"),
            modules,
        };

        let program = Inliner::inline(program);

        assert_eq!(program.modules.len(), 1);
        assert_eq!(
            program
                .modules
                .get(&String::from("main"))
                .unwrap()
                .functions
                .get(
                    &FunctionKey::with_id("main")
                        .signature(Signature::new().outputs(vec![Type::FieldElement]))
                )
                .unwrap(),
            &TypedFunctionSymbol::Here(TypedFunction {
                arguments: vec![],
                statements: vec![
                    TypedStatement::Definition(
                        TypedAssignee::Identifier(Variable::field_element("_0")),
                        FieldElementExpression::Number(FieldPrime::from(42)).into()
                    ),
                    TypedStatement::Return(vec![FieldElementExpression::Identifier(String::from(
                        "_0"
                    ))
                    .into(),])
                ],
                signature: Signature::new().outputs(vec![Type::FieldElement]),
            })
        );
    }

    #[test]
    fn multi_def_from_same_module() {
        // // main
        // def foo() -> (field):
        //     return 42
        // def main() -> (field):
        //     field a = foo()
        //     return a

        // inlined
        // def main() -> (field)
        //     field _0 = 42
        //     return _0

        let main = TypedModule {
            functions: vec![
                (
                    FunctionKey::with_id("main")
                        .signature(Signature::new().outputs(vec![Type::FieldElement])),
                    TypedFunctionSymbol::Here(TypedFunction {
                        arguments: vec![],
                        statements: vec![
                            TypedStatement::MultipleDefinition(
                                vec![Variable::field_element("a")],
                                TypedExpressionList::FunctionCall(
                                    FunctionKey::with_id("foo").signature(
                                        Signature::new().outputs(vec![Type::FieldElement]),
                                    ),
                                    vec![],
                                    vec![Type::FieldElement],
                                ),
                            ),
                            TypedStatement::Return(vec![FieldElementExpression::Identifier(
                                String::from("a"),
                            )
                            .into()]),
                        ],
                        signature: Signature::new().outputs(vec![Type::FieldElement]),
                    }),
                ),
                (
                    FunctionKey::with_id("foo")
                        .signature(Signature::new().outputs(vec![Type::FieldElement])),
                    TypedFunctionSymbol::Here(TypedFunction {
                        arguments: vec![],
                        statements: vec![TypedStatement::Return(vec![
                            FieldElementExpression::Number(FieldPrime::from(42)).into(),
                        ])],
                        signature: Signature::new().outputs(vec![Type::FieldElement]),
                    }),
                ),
            ]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let modules: HashMap<_, _> = vec![(String::from("main"), main)].into_iter().collect();

        let program = TypedProgram {
            main: String::from("main"),
            modules,
        };

        let program = Inliner::inline(program);

        assert_eq!(program.modules.len(), 1);
        assert_eq!(
            program
                .modules
                .get(&String::from("main"))
                .unwrap()
                .functions
                .get(
                    &FunctionKey::with_id("main")
                        .signature(Signature::new().outputs(vec![Type::FieldElement]))
                )
                .unwrap(),
            &TypedFunctionSymbol::Here(TypedFunction {
                arguments: vec![],
                statements: vec![
                    TypedStatement::Definition(
                        TypedAssignee::Identifier(Variable::field_element("_0")),
                        FieldElementExpression::Number(FieldPrime::from(42)).into()
                    ),
                    TypedStatement::Return(vec![FieldElementExpression::Identifier(String::from(
                        "_0"
                    ))
                    .into(),])
                ],
                signature: Signature::new().outputs(vec![Type::FieldElement]),
            })
        );
    }

    #[test]
    fn recursive_call_in_other_module() {
        // // main
        // def main(field a) -> (field):
        //     return id(id(a))

        // // id
        // def main(field a) -> (field)
        //     return a

        let main = TypedModule {
            functions: vec![
                (
                    FunctionKey::with_id("main").signature(
                        Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement]),
                    ),
                    TypedFunctionSymbol::Here(TypedFunction {
                        arguments: vec![Parameter::private(Variable::field_element("a"))],
                        statements: vec![TypedStatement::Return(vec![
                            FieldElementExpression::FunctionCall(
                                FunctionKey::with_id("id").signature(
                                    Signature::new()
                                        .inputs(vec![Type::FieldElement])
                                        .outputs(vec![Type::FieldElement]),
                                ),
                                vec![FieldElementExpression::FunctionCall(
                                    FunctionKey::with_id("id").signature(
                                        Signature::new()
                                            .inputs(vec![Type::FieldElement])
                                            .outputs(vec![Type::FieldElement]),
                                    ),
                                    vec![FieldElementExpression::Identifier(String::from("a"))
                                        .into()],
                                )
                                .into()],
                            )
                            .into(),
                        ])],
                        signature: Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement]),
                    }),
                ),
                (
                    FunctionKey::with_id("id").signature(
                        Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement]),
                    ),
                    TypedFunctionSymbol::There(
                        FunctionKey::with_id("main").signature(
                            Signature::new()
                                .inputs(vec![Type::FieldElement])
                                .outputs(vec![Type::FieldElement]),
                        ),
                        String::from("id"),
                    ),
                ),
            ]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let id = TypedModule {
            functions: vec![(
                FunctionKey::with_id("main").signature(
                    Signature::new()
                        .inputs(vec![Type::FieldElement])
                        .outputs(vec![Type::FieldElement]),
                ),
                TypedFunctionSymbol::Here(TypedFunction {
                    arguments: vec![Parameter::private(Variable::field_element("a"))],
                    statements: vec![TypedStatement::Return(vec![
                        FieldElementExpression::Identifier(String::from("a")).into(),
                    ])],
                    signature: Signature::new()
                        .inputs(vec![Type::FieldElement])
                        .outputs(vec![Type::FieldElement]),
                }),
            )]
            .into_iter()
            .collect(),
            imports: vec![],
        };

        let modules: HashMap<_, _> = vec![(String::from("main"), main), (String::from("id"), id)]
            .into_iter()
            .collect();

        let program: TypedProgram<FieldPrime> = TypedProgram {
            main: String::from("main"),
            modules,
        };

        let program = Inliner::inline(program);

        assert_eq!(program.modules.len(), 1);
        assert_eq!(
            program
                .modules
                .get(&String::from("main"))
                .unwrap()
                .functions
                .get(
                    &FunctionKey::with_id("main").signature(
                        Signature::new()
                            .inputs(vec![Type::FieldElement])
                            .outputs(vec![Type::FieldElement])
                    )
                )
                .unwrap(),
            &TypedFunctionSymbol::Here(TypedFunction {
                arguments: vec![Parameter::private(Variable::field_element("_0"))],
                statements: vec![
                    TypedStatement::Definition(
                        TypedAssignee::Identifier(Variable::field_element("_1")),
                        FieldElementExpression::Identifier(String::from("_0")).into()
                    ),
                    TypedStatement::Definition(
                        TypedAssignee::Identifier(Variable::field_element("_2")),
                        FieldElementExpression::Identifier(String::from("_1")).into()
                    ),
                    TypedStatement::Return(vec![FieldElementExpression::Identifier(String::from(
                        "_2"
                    ))
                    .into(),])
                ],
                signature: Signature::new()
                    .inputs(vec![Type::FieldElement])
                    .outputs(vec![Type::FieldElement]),
            })
        );
    }
}