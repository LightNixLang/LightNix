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
                                 | use_declare
                                 | let_statement
                                 | assign_statement
                                 | function_define

        import_statement     ::= "import" string_literal

        enum_define          ::= "enum" literal "{" [ lf ] { literal lf_or_comma } "}"

        type_define          ::= "type" literal type_define_block
        type_define_block    ::= "{" [ lf ] { type_define_element lf_or_comma } "}"
        type_define_element  ::= [ mutation_policy ] literal ":" ( type_define_block | type_info )

        use_declare          ::= "use" "[" [ lf ] { literal lf_or_comma } "]"

        let_statement        ::= [ "declare" ] "let" [ mutation_policy ] literal [ ":" type_info ] [ "=" [ lf ] expression ]

        mutation_policy      ::= "readonly"
                                 | "tunable" [ "(" "cost" "=" integer_literal ")" ]

        assign_statement     ::= expression "=" expression

        function_define      ::= function_attribute "function" literal function_arguments [ function_return_type ] block
        function_attribute   ::= "inline" | "opaque"
        function_arguments   ::= "(" [ lf ] { function_argument [ lf_or_comma ] } ")"
        function_argument    ::= literal ":" type_info
        function_return_type ::= "->" type_info

        block                ::= "{" statements "}"

        expression           ::= or_expr | if_expression | return_expression

        if_expression        ::= if_branch { "else" ( if_branch | block ) }
        if_branch            ::= "if" expression block

        return_expression    ::= "return" [ expression ]

        or_expr              ::= and_expr { "or" and_expr }
        and_expr             ::= equ_or_ine_expr { "and" equ_or_ine_expr }
        equ_or_ine_expr      ::= les_or_gre_expr [ ( "==" | "!=" ) les_or_gre_expr ]
        les_or_gre_expr      ::= add_or_sub_expr [ ( "<" | ">" | "<=" | ">=" ) add_or_sub_expr ]
        add_or_sub_expr      ::= mul_or_div_expr { ( "+" | "-" ) mul_or_div_expr }
        mul_or_div_expr      ::= factor { ( "*" | "/" ) factor }
        factor               ::= ( "+" | "-" ) primary | primary

        primary              ::= value { ( "." | "::" ) [ lf ] literal [ function_call ] }
        value                ::= array
                                 | literal [ function_call ]
                                 | numeric_literal
                                 | string_literal
                                 | "true"
                                 | "false"
                                 | "null"
        array                ::= "[" [ lf ] { value lf_or_comma } "]"

        function_call        ::= "(" [ lf ] { expression lf_or_comma } ")"

        type_info            ::= literal [ "<" type_info ">" ]

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
