import base64
content = base64.b64decode("Ly8gVGVzdAp1c2Ugc3RkOjpmczo6RmlsZTsKZm4gbWFpbigpIHt9Cg==").decode()
with open("src-tauri/src/audio_decode.rs", "w") as f: f.write(content)
print("Decoded and written")