#[cfg(test)]
mod grammar {
    use bnf_rules::bnf_rules_macro::bnf_rules;

    // This is an LR(1) parser generator, used for maintain quality.
    // If the specified grammar is ambiguous, compilation is aborted with conflict.
    // Usage : https://github.com/bea4dev/bnf_rules
    bnf_rules! {
        #[generate_code = false]

        source               ::= statements

        statements           ::= [ lf_or_semicolons ] { statement lf_or_semicolons }
        statement            ::= import_statement
                                 | enum_define
                                 | type_define
                                 | interface_define
                                 | implements_define
                                 | use_declare
                                 | let_statement
                                 | assert_statement
                                 | assign_statement
                                 | function_define
                                 | expression_statement

        expression_statement ::= if_expression
                                 | match_expression
                                 | return_expression
                                 | throw_expression

        import_statement     ::= "import" ( string_literal
                                            | import_names "from" string_literal
                                            | "*" "as" literal "from" string_literal )
        import_names         ::= "{" [ lf ] { import_name lf_or_comma } "}"
        import_name          ::= literal [ "as" literal ]

        enum_define          ::= [ "export" ] "enum" literal ( enum_symbolic_body | ":" type_info enum_repr_body )
        enum_symbolic_body   ::= "{" [ lf ] { literal lf_or_comma } "}"
        enum_repr_body       ::= "{" [ lf ] { enum_repr_variant lf_or_comma } "}"
        enum_repr_variant    ::= literal "=" expression

        type_define          ::= [ "export" ] "type" literal [ generic_parameters ] [ lf ]
                                 [ where_clause ] type_define_block
        type_define_block    ::= "{" [ lf ] { type_define_element lf_or_comma } "}"
        type_define_element  ::= [ mutation_policy ] literal ":" ( type_define_block | type_info )

        interface_define     ::= [ "export" ] "interface" literal [ generic_parameters ] [ lf ]
                                 [ where_clause ] interface_block
        interface_block      ::= "{" [ lf_or_semicolons ]
                                 { method_define lf_or_semicolons } "}"

        implements_define    ::= "implements" [ generic_parameters ] type_info "for" type_info [ lf ]
                                 [ where_clause ] implements_block
        implements_block     ::= "{" [ lf_or_semicolons ]
                                 { method_define lf_or_semicolons } "}"

        use_declare          ::= "use" "[" [ lf ] { literal lf_or_comma } "]"

        let_statement        ::= [ "export" ] [ "declare" ] "let" [ mutation_policy ] literal [ ":" type_info ] [ "=" [ lf ] expression ]

        assert_statement     ::= "assert" expression "," [ lf ] expression

        mutation_policy      ::= "readonly"
                                 | "tunable" [ "(" "cost" "=" integer_literal ")" ]

        assign_statement     ::= primary "=" [ lf ] assign_value
        assign_value         ::= expression | nested_assignment
        nested_assignment    ::= "{" [ lf ] { nested_assignment_field lf_or_comma } "}"
        nested_assignment_field ::= literal "=" [ lf ] assign_value

        function_define      ::= [ "export" ] method_define
        method_define        ::= function_attribute "function" literal [ generic_parameters ]
                                 function_arguments [ function_return_type ] [ lf ]
                                 [ where_clause ] block
        function_attribute   ::= "inline" | "opaque"
        function_arguments   ::= "(" [ lf ] [ "this" [ lf_or_comma ] ]
                                 { function_argument [ lf_or_comma ] } ")"
        function_argument    ::= literal ":" type_info
        function_return_type ::= "->" type_info

        generic_parameters   ::= "<" [ lf ] { generic_parameter [ lf_or_comma ] } ">"
        generic_parameter    ::= literal [ ":" interface_bound { "+" interface_bound } ]

        where_clause         ::= "where" [ lf ] { where_predicate [ lf_or_comma ] }
        where_predicate      ::= type_info ":" interface_bound { "+" interface_bound }
        interface_bound      ::= type_info

        block                ::= "{" statements "}"

        expression           ::= elvis_expr
                                 | if_expression
                                 | match_expression
                                 | return_expression
                                 | throw_expression
                                 | closure_expression

        closure_expression   ::= function_attribute "|" [ lf ]
                                 { closure_parameter [ lf_or_comma ] } "|"
                                 [ "->" type_info ] [ lf ] "=>" [ lf ] closure_body
        closure_parameter    ::= literal [ ":" type_info ]
        closure_body         ::= expression | block

        elvis_expr           ::= or_expr [ "?:" expression ]

        if_expression        ::= if_branch { "else" ( if_branch | block ) }
        if_branch            ::= "if" expression block

        match_expression     ::= "match" expression "{" [ lf ] { match_arm lf_or_comma } "}"
        match_arm            ::= match_pattern "=>" expression
        match_pattern        ::= "some" "(" match_pattern ")"
                                 | "null"
                                 | "_"
                                 | literal [ "::" literal ]

        return_expression    ::= "return" [ expression ]
        throw_expression     ::= "throw" expression

        or_expr              ::= and_expr { "or" and_expr }
        and_expr             ::= equ_or_ine_expr { "and" equ_or_ine_expr }
        equ_or_ine_expr      ::= les_or_gre_expr [ ( "==" | "!=" ) les_or_gre_expr ]
        les_or_gre_expr      ::= add_or_sub_expr [ ( "<" | ">" | "<=" | ">=" ) add_or_sub_expr ]
        add_or_sub_expr      ::= mul_or_div_expr { ( "+" | "-" ) mul_or_div_expr }
        mul_or_div_expr      ::= factor { ( "*" | "/" ) factor }
        factor               ::= ( "+" | "-" ) primary | primary

        primary              ::= value { ( "." | "?." | "::" ) [ lf ] literal
                                           [ type_arguments ] [ function_call ] }
        value                ::= array
                                 | literal [ type_arguments ] [ function_call ]
                                 | some_value
                                 | numeric_literal
                                 | string_literal
                                 | "true"
                                 | "false"
                                 | "null"
        array                ::= "[" [ lf ] { value lf_or_comma } "]"
        some_value           ::= "some" "(" expression ")"

        function_call        ::= "(" [ lf ] { expression lf_or_comma } ")"
        type_arguments       ::= ":" "<" [ lf ] { type_argument [ lf_or_comma ] } ">"
        type_argument        ::= type_info | "_"

        type_info            ::= literal [ "<" [ lf ] { type_info [ lf_or_comma ] } ">" ] [ "?" ]

        literal              ::= r"\w+"

        numeric_literal      ::= r"[\d_]+(\.[\d_]+)?([eE][+-]?[\d_]+)?"
        integer_literal      ::= r"[\d_]+"

        string_literal       ::= r#""([^"\\]|\\.)*""# | r"'([^'\\]|\\.)*'"

        lf_or_comma          ::= lf | ","

        lf_or_semicolons     ::= lf_or_semicolon { lf_or_semicolon }
        lf_or_semicolon      ::= lf | ";"
        lf                   ::= r"(\n|\r|\r\n)+"
    }
}
