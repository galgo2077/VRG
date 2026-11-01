# VRG — Instalar, configurar y comprobar

> **Guía de la versión 0.5.0 — Linux x86_64 y Codex.** Las comprobaciones de distribución y artefactos públicos se registran en el [release](https://github.com/galgo2077/VRG/releases/tag/v0.5.0). La versión incorpora `setup`, `doctor` y memoria adaptativa opt-in.

El [tag histórico v0.4.0-f4](https://github.com/galgo2077/VRG/releases/tag/v0.4.0-f4) permanece intacto; sus certificaciones no se repiten ni se transfieren a esta versión. macOS y Windows quedan fuera de este alcance.

[Guía web](../es/setup.html) · [English](SETUP.md) · [Memoria adaptativa](ADAPTIVE_MEMORY.es.md) · [Manual avanzado](../PUBLIC_INSTALL.md)

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

## 2. Descargar o compilar 0.5.0

**Binario Linux x86_64 0.5.0:** comprueba `SHA256SUMS.txt` antes de extraer. SHA-256 del paquete: `55f011dffd88147c3988ad9e2cb11a58faa7da73d0108649262f6ecf3dcc8534`. Requisitos medidos: GLIBC 2.39, GLIBCXX 3.4.31 y CXXABI 1.3.11; host validado glibc 2.41.

```sh
VRG_DOWNLOAD_DIR="$(mktemp -d /tmp/vrg-download.XXXXXXXX)" || exit 1
( cd "$VRG_DOWNLOAD_DIR" &&
  curl -fLO https://github.com/galgo2077/VRG/releases/download/v0.5.0/vrg-v0.5.0-linux-x86_64.tar.gz &&
  curl -fLO https://github.com/galgo2077/VRG/releases/download/v0.5.0/SHA256SUMS.txt &&
  sha256sum -c SHA256SUMS.txt &&
  tar -xzf vrg-v0.5.0-linux-x86_64.tar.gz &&
  ./vrg-v0.5.0-linux-x86_64/vrg --help )
```



```sh
git clone --branch v0.5.0 --depth 1 https://github.com/galgo2077/VRG.git
cd VRG
ORT_LIB_LOCATION=/ruta/absoluta/onnxruntime-verificado ORT_OFFLINE=1 \
  CARGO_NET_OFFLINE=true cargo build --release --locked --offline
test -x target/release/vrg
./target/release/vrg --help
```

La compilación requiere Rust 1.92+; el candidato usa Rust 1.98. Prepara por separado las dependencias exactas de `Cargo.lock` y la biblioteca nativa ONNX Runtime 1.31.0. `--offline` no instala dependencias faltantes. La preparación reproducible está en [el workflow Linux](https://github.com/galgo2077/VRG/blob/v0.5.0/.github/workflows/adaptive-linux-release.yml), con distribución nativa y avisos MIT/terceros fijados por hash.

Con la biblioteca verificada en un directorio propio, compila sin red:

```sh
ORT_LIB_LOCATION=/ruta/absoluta/onnxruntime-verificado ORT_OFFLINE=1 \
  CARGO_NET_OFFLINE=true cargo build --release --locked --offline
```

**Alternativa crates.io, cuando 0.5.0 esté publicada:**

```sh
cargo install vrg --version 0.5.0 --locked
vrg --version
```

`cargo install` compila localmente; puede descargar crates y el runtime nativo. No descarga el binario oficial ni garantiza funcionar sin red. El archivo Linux ya compilado evita esa preparación de build. No se incluye el modelo E5; solo hace falta para búsqueda semántica.

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

**Compatibilidad actual con asistentes:** solamente Codex. No hay soporte oficial ni validación para otros clientes de IA, aunque implementen MCP. La indexación y búsqueda mediante el CLI funcionan sin Codex.

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

Hay un ejemplo Unix en `examples/codex/hooks.json.example` y un supervisor `scripts/vrg_hook_observer.py`. Son ejemplos manuales. Setup puede crear recursos de hooks del proyecto; sus definiciones reales requieren revisión y confianza en Codex.

Configura `VRG_HOOK_EXECUTABLE` y `VRG_HOOK_EVIDENCE_DIR` con rutas canónicas privadas y existentes. Revisa las definiciones exactas de `SessionStart`, `UserPromptSubmit` y `Stop` en `/hooks`; confía únicamente lo que hayas aprobado y verifica los eventos reales en otra sesión. El supervisor portable no hereda la certificación del supervisor original.

El MCP es de solo lectura. Codex puede escribir documentación compacta en Markdown **expresamente autorizado**, pero no conversaciones completas ni notas privadas por defecto.

## 6. Checklist de verificación

| Prueba | Evidencia requerida |
|---|---|
| Fuente | Tag `v0.5.0` y revisión correcta |
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

Las comprobaciones F4 históricas pertenecen a su revisión y entorno originales. No son un paso rutinario de instalación ni una nueva certificación Adaptive Memory.

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

## 8. Distribución y operación 0.5.0

- [Release y código v0.5.0](https://github.com/galgo2077/VRG/releases/tag/v0.5.0).
- [Binario Linux x86_64](https://github.com/galgo2077/VRG/releases/download/v0.5.0/vrg-v0.5.0-linux-x86_64.tar.gz) y [SHA256SUMS](https://github.com/galgo2077/VRG/releases/download/v0.5.0/SHA256SUMS.txt).
- [Paquete vrg 0.5.0](https://crates.io/crates/vrg/0.5.0). Consulta en el release la comprobación de descarga independiente.

### Setup y doctor

Ejecuta `vrg setup` para elegir opciones en una terminal. Para un perfil textual de proyecto autorizado, revisa primero la vista previa:

```sh
vrg --config /ruta/absoluta/perfil.toml setup \
  --project /ruta/absoluta/proyecto --root /ruta/absoluta/notas-autorizadas \
  --data-dir /ruta/absoluta/derivados --text-only --codex --allow-remote-context \
  --memory-dir /ruta/absoluta/proyecto/vrg-memory --non-interactive --yes --offline --dry-run
vrg --config /ruta/absoluta/perfil.toml doctor --json
```

Retira `--dry-run` para aplicar únicamente el plan autorizado. `--yes` no concede confianza de hooks. `--allow-remote-context` autoriza recibir los fragmentos en el modelo Codex configurado; usa `--cli-only` si no autorizas esa transferencia. `--root` es repetible. `--model-dir` selecciona un modelo existente; `--download-model` pide descarga explícita y no funciona como descarga nueva con `--offline`.

Si se crean hooks, revisa sus comandos en `/hooks` y comprueba eventos reales en una sesión nueva. Doctor es pasivo por defecto; `--check-runtime` usa un cliente MCP aislado, no acredita una sesión Codex. `doctor --fix --dry-run` muestra reparaciones propias reconocidas; `--fix` las aplica sin ampliar permisos. `setup --recover /ruta/absoluta/transaccion --dry-run` prepara recuperación; conflictos posteriores requieren revisión y no justifican borrar notas o bases.

Para empezar sin notas reales, usa `setup --project /ruta/absoluta/proyecto --synthetic --text-only --cli-only --non-interactive --yes --offline`; solo se admite `sample.md`. La memoria adaptativa se activa por elección explícita `--adaptive-memory`; consulta [HOT/WARM/COLD, propuestas, presupuesto y migración](ADAPTIVE_MEMORY.es.md). MCP permanece de solo lectura. No reemplaces el servidor estable antes de validar el perfil y su rollback.
