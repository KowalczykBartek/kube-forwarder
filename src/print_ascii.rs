static ASCII_ROCKET: &str = r#"
			                                                   ,:
			                                                 ,' |
			                                                /   :
			                                             --'   /
			                                             \\/ />/
			                                             / /_\\
			                                          __/   /
			                                          )'-. /
			                                          ./  :\\
			                                           /.' '
			                                         '/'
			                                         +
			                                        '
			                                      `.
			                                  .-"-
			                                 (    |
			                              . .-'  '.
			                             ( (.   )8:
			                         .'    / (_  )
			                          _. :(.   )8P  `
			                      .  (  `-' (  `.   .
			                       .  :  (   .a8a)
			                      /_`( "a `a. )"'
			                  (  (/  .  ' )=='
			                 (   (    )  .8"   +
			                   (`'8a.( _(   (
			                ..-. `8P    ) `  )  +
			              -'   (      -ab:  )
			            '    _  `    (8P"Ya
			          _(    (    )b  -`.  ) +
			         ( 8)  ( _.aP" _a   \\( \\   *
			       +  )/    (8P   (88    )  )
			          (a:f   "     `"       `
"#;

pub fn print_rocket_std_output() {
    log::info!("port-forwarder is running");
    log::info!("{}", ASCII_ROCKET);
}