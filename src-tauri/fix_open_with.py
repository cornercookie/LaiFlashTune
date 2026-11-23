with open('src/open_with.rs', 'r') as f:
    lines = f.readlines()

# Find the end of impl Sanitizer and rebuild from there
new_lines = []
i = 0
in_sanitizer_impl = False
while i < len(lines):
    line = lines[i]
    if 'impl Sanitizer {' in line:
        in_sanitizer_impl = True
    if in_sanitizer_impl and line.strip() == '}' and i+1 < len(lines) and lines[i+1].strip() == '}':
        # End of impl Sanitizer - keep this line and the next, then add our new code
        new_lines.append(line)  # }
        new_lines.append(lines[i+1])  # }
        i += 2
        break
    new_lines.append(line)
    i += 1

# Add the missing code
new_lines.append('\n')
new_lines.append('impl Drop for Sanitizer {\n')
new_lines.append('    fn drop(&mut self) {\n')
new_lines.append('        let _ = self.finish();\n')
new_lines.append('    }\n')
new_lines.append('}\n')
new_lines.append('\n')
new_lines.append('/// 取消標記 trait，供 spawn_stderr_sanitizer 使用（SE3-4）\n')
new_lines.append('pub trait Cancelled: Send + Sync {\n')
new_lines.append('    fn is_cancelled(&self) -> bool;\n')
new_lines.append('}\n')
new_lines.append('\n')
new_lines.append('/// 共用的 stderr 清洗執行緒生成器（SE3-4）：將子程序 stderr 讀取、清洗、記錄的重複邏輯提取為 helper。\n')
new_lines.append('/// - 若提供 cancel，執行緒會在 cancel 觸發時提前退出並仍呼叫 finish()。\n')
new_lines.append('/// - 呼叫端只需 let _handle = spawn_stderr_sanitizer(stderr, cancel.clone()); 結束時 join 即可。\n')
new_lines.append('pub fn spawn_stderr_sanitizer(\n')
new_lines.append('    stderr: Option<ChildStderr>,\n')
new_lines.append('    cancel: Option<std::sync::Arc<dyn Cancelled>>,\n')
new_lines.append(') -> JoinHandle<()> {\n')
new_lines.append('    std::thread::spawn(move || {\n')
new_lines.append('        use std::io::Read;\n')
new_lines.append('        let mut buf = vec![0u8; 4096];\n')
new_lines.append('        let mut sanitizer = Sanitizer::new();\n')
new_lines.append('        if let Some(mut stderr) = stderr {\n')
new_lines.append('            loop {\n')
new_lines.append('                if let Some(ref c) = cancel {\n')
new_lines.append('                    if c.is_cancelled() {\n')
new_lines.append('                        break;\n')
new_lines.append('                    }\n')
new_lines.append('                }\n')
new_lines.append('                match stderr.read(&mut buf) {\n')
new_lines.append('                    Ok(0) => break,\n')
new_lines.append('                    Ok(n) => {\n')
new_lines.append('                        let chunk_text = String::from_utf8_lossy(&buf[..n]);\n')
new_lines.append('                        sanitizer.sanitize_chunk(&chunk_text);\n')
new_lines.append('                    }\n')
new_lines.append('                    Err(_) => break,\n')
new_lines.append('                }\n')
new_lines.append('            }\n')
new_lines.append('        }\n')
new_lines.append('        // 確保所有結束路徑（cancel、EOF、Err）均呼叫 finish()\n')
new_lines.append('        let cleaned = sanitizer.finish();\n')
new_lines.append('        if !cleaned.is_empty() {\n')
new_lines.append('            log_line_raw(&cleaned);\n')
new_lines.append('        }\n')
new_lines.append('    })\n')
new_lines.append('}\n')
new_lines.append('\n')
new_lines.append('/// 日誌輪替互斥鎖（SE3-10）：保護 rotate_log_if_needed 關鍵區，避免多執行緒同時輪替導致檔案損壞或遺漏清理。\n')
new_lines.append('static LOG_ROTATE_MUTEX: Mutex<()> = Mutex::new(());\n')
new_lines.append('\n')

# Now skip the corrupted lines until we hit log_line_raw
while i < len(lines):
    if 'pub fn log_line_raw' in lines[i]:
        break
    i += 1

# Add the rest of the file
while i < len(lines):
    new_lines.append(lines[i])
    i += 1

with open('src/open_with.rs', 'w') as f:
    f.writelines(new_lines)
print('Fixed open_with.rs')