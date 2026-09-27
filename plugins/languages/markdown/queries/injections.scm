; Written for nib. The code of a fenced block, in the language its info
; string names ("rust", or a file type such as "rs").
(fenced_code_block
  (info_string
    (language) @injection.language)
  (code_fence_content) @injection.content)

; A block with no info string: in the language of what the Markdown is in,
; as rustdoc reads the blocks in Rust's doc comments. nib's injection.parent
; leaves a Markdown file's own blocks as text. The anchors keep out blocks
; whose info string sits between the fence and the rest.
(fenced_code_block
  (fenced_code_block_delimiter)
  .
  (block_continuation)
  .
  (code_fence_content) @injection.content
  (#set! injection.parent))

(fenced_code_block
  (fenced_code_block_delimiter)
  .
  (code_fence_content) @injection.content
  (#set! injection.parent))

; A block whose info string holds only rustdoc's attributes ("ignore",
; "no_run,should_panic", "edition2021") is Rust to rustdoc; in the language
; of what the Markdown is in, as a block with none.
(fenced_code_block
  (info_string
    (language) @_attributes)
  (code_fence_content) @injection.content
  (#match? @_attributes "^(ignore|no_run|should_panic|compile_fail|standalone_crate|test_harness|edition[0-9]+)(,(ignore|no_run|should_panic|compile_fail|standalone_crate|test_harness|edition[0-9]+))*$")
  (#set! injection.parent))

; Front matter.
((minus_metadata) @injection.content
 (#set! injection.language "yaml"))

((plus_metadata) @injection.content
 (#set! injection.language "toml"))

; The inline elements, in their own grammar.
([(inline) (pipe_table_cell)] @injection.content
 (#set! injection.language "markdown_inline"))
