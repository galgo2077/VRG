# VRG — Instalar, configurar y comprobar

> **Estado (1 de noviembre de 2026).** [`v0.4.0-f4`](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) es un **release de código fuente**: contiene Rust y `Cargo.lock`, pero **no binarios adjuntos**. No hay una publicación en crates.io verificada para este proyecto: no asumas que `cargo install vrg` instala este VRG. El tag identifica el hito F4; `Cargo.toml` continúa declarando `0.1.0`. macOS y Windows no están certificados.

[Guía web](../es/setup.html) · [English](SETUP.md) · [Manual avanzado](../PUBLIC_INSTALL.md) · [GitHub](https://github.com/galgo2077/VRG)

## 1. Requisitos

Linux es la ruta documentada; Void Linux es el entorno local donde se realizaron las validaciones comunicadas. Instala:

- Git.
- Rust **1.92 o superior** y Cargo mediante [rustup](https://rustup.rs) o la [guía oficial de Rust](https://www.rust-lang.org/tools/install).
- Compilador C/C++, `make` y `pkg-config` cuando los requieran las dependencias.

```sh
git --version
rustc --version
cargo --version
cc --version
pkg-config --version
```

En Void Linux puedes consultar nombres de paquetes mediante `xbps-query -Rs`; no supongas que un comando de instalación se aplica a todas las distribuciones.

## 2. Descargar y compilar (DISPONIBLE)

```sh
git clone --branch v0.4.0-f4 --depth 1 https://github.com/galgo2077/VRG.git
cd VRG
cargo build --release --locked
test -x target/release/vrg
./target/release/vrg --help
```

La primera compilación puede necesitar red para descargar las dependencias fijadas. Cuando ya están en caché se puede ejecutar:

```sh
cargo build --release --locked --offline
```

**Alternativa no verificada en una instalación externa limpia:**

```sh
cargo install --git https://github.com/galgo2077/VRG.git --tag v0.4.0-f4 --locked
vrg --help
```

Ese comando **compila desde GitHub**; no descarga un binario ni usa crates.io. Si falla, utiliza la compilación del checkout. El ejecutable puede reportar versión `0.1.0`: el tag `v0.4.0-f4` es la versión del hito del proyecto.

## 3. Comprobar con notas sintéticas

No autorices tus vaults reales solo para comprobar la instalación. Tras compilar, desde el checkout ejecuta:

```sh
DEMO_BASE="$(mktemp -d /tmp/vrg-demo.XXXXXXXX)" || { echo "Cannot create demo directory" >&2; exit 1; }
mkdir "$DEMO_BASE/notes"
printf '# Demo\n\nLa brujula zafiro azul es una prueba sintetica.\n' > "$DEMO_BASE/notes/demo.md"
DEMO_CFG="$DEMO_BASE/config.toml"
cat > "$DEMO_CFG" <<EOF
data_dir = "$DEMO_BASE/data"
capture_session_events = false

[[roots]]
id = "demo"
path = "$DEMO_BASE/notes"
allowed_files = ["demo.md"]
EOF
./target/release/vrg --config "$DEMO_CFG" status
./target/release/vrg --config "$DEMO_CFG" index
./target/release/vrg --config "$DEMO_CFG" search "brújula zafiro" --root demo --text-only
```

Comprueba que devuelve `demo.md` con fuente/revisión y que el Markdown original no cambió. `allowed_files` restringe la indexación a ese archivo. No necesitas un modelo semántico para la búsqueda textual.

Para comprobar la sincronización incremental, modifica **solo** el archivo sintético y consulta otra vez dentro del perfil de prueba. Una indexación manual no demuestra, por sí sola, un watcher automático.

## 4. Modelo semántico opcional

La validación local usó `intfloat/multilingual-e5-small`. El proyecto no lo descarga ni lo incluye. La carpeta del modelo necesita:

- `model.onnx`
- `tokenizer.json`
- `config.json`
- `special_tokens_map.json`
- `tokenizer_config.json`

Confirma revisión, hashes y licencia antes de activar `semantic_model`. La búsqueda textual funciona sin él. Un modelo local para embeddings no convierte en local el modelo de Codex.

## 5. MCP para Codex

Empieza con datos sintéticos. Crea `.codex/config.toml` de proyecto usando rutas absolutas reales:

```toml
[mcp_servers.vrg]
command = "/ruta/absoluta/a/VRG/target/release/vrg"
args = ["--config", "/ruta/absoluta/demo.toml", "mcp", "serve"]
cwd = "/ruta/absoluta/a/VRG"
enabled = true
required = true
enabled_tools = [
  "vrg_status",
  "vrg_search",
  "vrg_read_source",
  "vrg_graph_neighbors"
]
```

El comando real es `vrg --config CONFIG mcp serve`. Abre una sesión nueva de Codex en el proyecto confiado, revisa `/mcp` y prueba `vrg_status`, búsqueda, lectura y grafo únicamente sobre fuentes de prueba autorizadas. El MCP puede devolver fragmentos al contexto de un modelo remoto: no uses notas personales sin autorización de transferencia.

### Hooks y autodocumentación

Hay un ejemplo Unix en `examples/codex/hooks.json.example` y un supervisor `scripts/vrg_hook_observer.py`. **No se instalan ni confían automáticamente**.

Configura `VRG_HOOK_EXECUTABLE` y `VRG_HOOK_EVIDENCE_DIR` con rutas canónicas privadas y existentes. Revisa las definiciones exactas de `SessionStart`, `UserPromptSubmit` y `Stop` en `/hooks`; confía únicamente lo que hayas aprobado y verifica los eventos reales en otra sesión. El supervisor portable no hereda la certificación del supervisor original.

El MCP es de solo lectura. Codex puede escribir documentación compacta en Markdown **expresamente autorizado**, pero no conversaciones completas ni notas privadas por defecto.

## 6. Checklist de verificación

| Prueba | Evidencia requerida |
|---|---|
| Fuente | Tag `v0.4.0-f4` y revisión correcta |
| Compilación | Existe `target/release/vrg` y responde `--help` |
| Configuración | Raíz sintética y almacenamiento aislado |
| Indexación | `demo.md` se indexa, fuente intacta |
| Búsqueda | Hit de texto con revisión atribuida |
| Modelo | `ready` solo cuando el modelo verificado está disponible |
| Grafo | Enlaces sintéticos sin inventar destinos |
| MCP | Cuatro herramientas llamadas desde Codex real |
| Hooks | Definiciones confiadas, eventos y salidas verificados |
| Privacidad | Ningún documento excluido devuelto |
| Frescura | Revisiones nuevas no etiquetadas como antiguas |
| Memoria | Una segunda sesión recupera documentación sintética |
| Recuperación | Backup y rollback solo con datos de prueba |

### Pruebas dirigidas del mantenedor

Estos casos pertenecen al código F4 completo. Requieren fixtures y dependencias adecuadas:

```sh
cargo test --locked --test codex_auto_sync f4_01_private_metadata_and_external_paths_are_excluded -- --exact
cargo test --locked --features f4-instrumentation --test codex_auto_sync f4_04_concurrent_publication_and_journal_recovery_are_deterministic -- --exact
cargo test --locked --test codex_auto_sync f4_05_mcp_exposes_read_only_tools_and_enforces_root_boundaries -- --exact
```

F4-03 necesita `VRG_F4_MODEL_DIR` y un modelo local de hash verificado. Usa `--offline` únicamente cuando todas las dependencias estén en caché. No uses datos reales para escenarios destructivos.

## 7. Errores comunes

| Error | Solución inicial |
|---|---|
| Falta Cargo | Instalar Rust/Cargo mediante rustup |
| Compilación nativa | Revisar el error de dependencia y las bibliotecas necesarias |
| No existe `vrg` | Revisar el build y el objetivo `[[bin]]` |
| Offline falla | Faltan crates en caché; obtener dependencias fijadas primero |
| Estado falla | Revisar configuración, rutas y permisos |
| `sync_pending` | Esperar reconciliación real, sin fingir datos frescos |
| MCP no aparece | Revisar `/mcp`, ejecutable y configuración efectiva |
| Hook `untrusted` | Revisar confianza en `/hooks` |
| Semántica no disponible | Buscar solo texto o instalar modelo verificado |

## 8. Estado de distribución

- **Código fuente:** [release v0.4.0-f4](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4).
- **Binarios listos:** ninguno adjunto al release.
- **crates.io:** sin publicación oficial verificada para este proyecto.
- **Versión de Cargo:** `0.1.0`; tag `v0.4.0-f4` = hito Fase 4.
- **Sistema validado:** Void Linux y casos locales; macOS/Windows requieren pruebas separadas.

Consulta también [manual de operación](../PUBLIC_INSTALL.md).
