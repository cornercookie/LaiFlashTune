import re

# Read the current file
with open('src-tauri/src/audio_decode.rs', 'r') as f:
    content = f.read()

# Fix common issues: add newlines after semicolons, braces, etc.
# This is a simple formatter to make the file readable and check brace balance

# First, let's add newlines after certain patterns
content = content.replace('; ', ';\n')
content = content.replace('{ ', '{\n')
content = content.replace('} ', '}\n')
content = content.replace('}else', '}\nelse')
content = content.replace('}else', '}\nelse')
content = content.replace('if ', '\nif ')
content = content.replace('loop {', 'loop {\n')
content = content.replace('match ', '\nmatch ')
content = content.replace('Ok(', '\nOk(')
content = content.replace('Err(', '\nErr(')
content = content.replace('let ', '\nlet ')
content = content.replace('let mut ', '\nlet mut ')
content = content.replace('pub fn ', '\npub fn ')
content = content.replace('fn ', '\nfn ')
content = content.replace('impl ', '\nimpl ')
content = content.replace('struct ', '\nstruct ')
content = content.replace('enum ', '\nenum ')
content = content.replace('pub struct ', '\npub struct ')
content = content.replace('pub enum ', '\npub enum ')
content = content.replace('const ', '\nconst ')
content = content.replace('static ', '\nstatic ')
content = content.replace('use ', '\nuse ')
content = content.replace('mod ', '\nmod ')

# Fix multiple newlines
content = re.sub(r'\n{3,}', '\n\n', content)

# Write back
with open('src-tauri/src/audio_decode.rs', 'w') as f:
    f.write(content)

print("Formatted file written")

# Check brace balance
open_braces = content.count('{')
close_braces = content.count('}')
print(f"Open braces: {open_braces}, Close braces: {close_braces}, Balance: {open_braces - close_braces}")

# Also check brackets and parens
open_parens = content.count('(')
close_parens = content.count(')')
print(f"Open parens: {open_parens}, Close parens: {close_parens}, Balance: {open_parens - close_parens}")

open_brackets = content.count('[')
close_brackets = content.count(']')
print(f"Open brackets: {open_brackets}, Close brackets: {close_brackets}, Balance: {open_brackets - close_brackets}")