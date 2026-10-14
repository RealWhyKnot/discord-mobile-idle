use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const LANGUAGE: u16 = 0x0409;
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{e2011457-1546-43c5-a5fe-008deee3d3f0}"/>
      <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;

fn entry(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, language: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&language.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn icon_resources(ico: &[u8]) -> Vec<u8> {
    let count = u16_at(ico, 4) as usize;
    let mut out = Vec::new();
    entry(&mut out, 0, 0, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend_from_slice(&0u16.to_le_bytes());
    group.extend_from_slice(&1u16.to_le_bytes());
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for i in 0..count {
        let at = 6 + 16 * i;
        let size = u32_at(ico, at + 8) as usize;
        let offset = u32_at(ico, at + 12) as usize;
        let id = (i + 1) as u16;
        entry(&mut out, RT_ICON, id, 0x1010, LANGUAGE, &ico[offset..offset + size]);
        group.extend_from_slice(&ico[at..at + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    entry(&mut out, RT_GROUP_ICON, 1, 0x1030, LANGUAGE, &group);
    out
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=packaging/icon.ico");
    println!("cargo:rerun-if-env-changed=IDLEBOT_VERSION");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows || !msvc {
        return;
    }
    let Ok(ico) = std::fs::read("packaging/icon.ico") else {
        return;
    };
    let dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let res = dir.join("icon.res");
    std::fs::write(&res, icon_resources(&ico)).expect("write icon.res");
    let manifest = dir.join("app.manifest");
    std::fs::write(&manifest, MANIFEST).expect("write app.manifest");
    println!("cargo:rustc-link-arg-bin=discord-mobile-idle={}", res.display());
    println!("cargo:rustc-link-arg-bin=discord-mobile-idle=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=discord-mobile-idle=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
