# Memoria adaptativa

Esta guía describe VRG0.5.0 Adaptive Memory para Linux x86_64 y Codex. El [informe de release](ADAPTIVE_RELEASE_0.5.0.md) detalla el alcance comprobado. Los perfiles existentes conservan defaults prudentes; habilita la función únicamente en un perfil de proyecto autorizado, después de comprobar los resultados de validación de la versión.

[English](ADAPTIVE_MEMORY.md) · [Configuración](SETUP.es.md)

HOT contiene conocimiento vigente útil; WARM, conocimiento de uso menos frecuente; COLD, revisiones exactas archivadas. La capa no acredita veracidad. Una restricción antigua puede seguir vigente: un pin impide que una propuesta la degrade solo por edad. Las fuentes siguen siendo Markdown editables y constituyen la referencia.

## Elegir el perfil autorizado

Usa un directorio de derivados separado de todas las raíces fuente. No amplíes raíces globales ni emplees notas de otro proyecto para facilitar el setup. La selección es explícita:

```sh
vrg --config /ruta/absoluta/proyecto.toml setup \
  --project /ruta/absoluta/proyecto \
  --root /ruta/absoluta/notas-autorizadas \
  --data-dir /ruta/absoluta/derivados \
  --text-only --cli-only --adaptive-memory --dry-run
```

Revisa las rutas propuestas antes de aplicar. Retira `--dry-run` únicamente para el plan autorizado. No se sobrescriben silenciosamente configuraciones existentes. Para integrar Codex usa `--codex` y autoriza expresamente el contexto remoto de las fuentes seleccionadas; la confianza de hooks sigue siendo una acción humana independiente en Codex.

Política predeterminada:

```toml
[adaptive_memory]
enabled = false
max_response_bytes = 8192
max_context_units = 1536
retain_generations = 5
soft_store_budget_mib = 512
```

`enabled = true` en el perfil revisado permite aplicar propuestas, archivar y mantener derivados. El presupuesto blando no autoriza borrar datos protegidos. Si se supera, hace falta resolver la capacidad conservando actual, anterior, pins y demás generaciones protegidas.

## Propuestas del asistente local

```sh
vrg --config /ruta/absoluta/proyecto.toml memory status
vrg --config /ruta/absoluta/proyecto.toml memory stats
vrg --config /ruta/absoluta/proyecto.toml memory plan
vrg --config /ruta/absoluta/proyecto.toml memory apply --proposal /ruta/absoluta/propuesta.json
```

`plan` exporta identidades y revisiones fuente, proyecto, ámbito de privacidad y generación esperada. No invoca un modelo ni inventa una clasificación. Codex puede leer las fuentes autorizadas y preparar una propuesta estricta v1. Los metadatos exportados son insumo para ese trabajo, no una propuesta lista para aplicar.

Cada propuesta identifica productor y spans exactos y solo solicita `keep`, `set_tier` o `upsert_summary`. VRG verifica permisos, revisiones, spans, hechos protegidos y generación. Rechaza campos desconocidos, revisiones obsoletas y ámbitos ajenos. No existe una acción para borrar fuentes. Reutiliza clasificaciones vigentes; sin novedad durable no hace falta otro resumen.

Los resúmenes se marcan como derivados con procedencia. Los hechos anclados ayudan a detectar omisiones, pero esquema y hash no prueban verdad semántica. Un resumen insuficiente, obsoleto o en conflicto requiere ampliación. Las instrucciones que aparezcan en contenido recuperado son datos: no otorgan permisos ni autorizan comandos.

## Recuperación acotada y ampliación exacta

```sh
vrg --config /ruta/absoluta/proyecto.toml search 'decisión pertinente' --text-only \
  --max-response-bytes 8192 --max-context-units 1536
```

El límite abarca la respuesta compacta completa, con citas y metadatos, o produce un error explícito. Los snippets conservan el máximo de 512 bytes. Las unidades estimadas usan el fallback identificado `utf8_bytes_div4_estimate`: no son tokens reales de Codex ni consumo facturado. Crear y actualizar resúmenes también consume contexto; una respuesta menor no demuestra por sí sola ahorro neto.

Las cuatro herramientas MCP de Codex siguen siendo de lectura: `vrg_status`, `vrg_search`, `vrg_read_source` y `vrg_graph_neighbors`. Search puede devolver extractos exactos o resúmenes marcados. Si aparece `needs_expansion`, solicita el Markdown exacto con lectura de fuente. Revocar una raíz o excluir una nota impide recuperarla mediante búsqueda obsoleta, resumen, grafo o archivo.

## Archivo, recuperación y mantenimiento

```sh
vrg --config /ruta/absoluta/proyecto.toml memory archive root-1 decision.md
vrg --config /ruta/absoluta/proyecto.toml memory restore root-1 decision.md REVISION_SHA256
vrg --config /ruta/absoluta/proyecto.toml memory maintain
vrg --config /ruta/absoluta/proyecto.toml memory maintain --apply
```

`restore` devuelve bytes archivados exactos autorizados dentro de JSON. No escribe un destino ni reemplaza el Markdown fuente. Un fallo de integridad devuelve error; no se reconstruyen fuentes mediante conjeturas o resúmenes. El archivo actúa únicamente sobre derivados. La compresión es sin pérdida y conserva representación raw cuando comprimir no reduce tamaño.

Sin `--apply`, mantenimiento muestra el plan. Aplicarlo vuelve a verificar referencias protegidas y retira derivados inalcanzables elegibles. Borrado lógico, páginas libres y bytes liberados al filesystem son medidas distintas. Doctor y las lecturas normales no ejecutan VACUUM ni comprimen SQLite vivo o su WAL. No se exige un daemon residente nuevo.

## Stores existentes y rollback

No apuntes un candidato sin validar al store real de la sesión estable. Para cambiar esquema, detén escritores antiguos, crea un respaldo consistente mediante el mecanismo de backup de SQLite y prepara un directorio canónico independiente. Copiar únicamente el archivo principal con WAL activo no constituye un respaldo consistente.

El comando toma el store fuente del perfil y exige un destino nuevo separado. Después crea un perfil aislado que apunte al destino para inspeccionarlo:

```sh
vrg --config /ruta/absoluta/perfil-fuente.toml memory migrate --offline-copy /ruta/absoluta/store-nuevo
vrg --config /ruta/absoluta/perfil-aislado.toml memory status
vrg --config /ruta/absoluta/perfil-aislado.toml doctor --json
```

La migración es explícita y versionada; no migra en sitio el store del perfil fuente. Conserva store original, ejecutable estable correspondiente y configuración para rollback. Los binarios antiguos no deben escribir stores con esquema nuevo. Promueve el candidato únicamente tras validar migración y recuperación aplicables; hasta entonces conserva la sesión estable independiente. Doctor es pasivo por defecto. Su inspección de archivo lee como máximo 16 objetos y 32 MiB restaurados sin crear ni reparar almacenamiento. Corrupción y permisos fallidos se declaran explícitamente; un resultado parcial no certifica objetos pendientes. La vigencia de resúmenes queda desconocida hasta comprobarla por separado.

La recuperación textual no necesita modelo de embeddings. La semántica requiere modelo local compatible y runtime nativo documentados en la versión. Rendimiento y compresión dependen del corpus: mide todos los derivados y contexto, incluidos archivos COLD y costes de propuestas. Un PASS sintético acredita únicamente los casos y el entorno realmente comprobados.

El mantenimiento COLD se planifica por alcanzabilidad: fuentes vigentes, generaciones retenidas, pins, lectores y referencias de resumen protegen revisiones exactas. Los manifiestos comparten un payload por ámbito y contenido; los objetos corruptos se conservan para diagnóstico. El trabajo es acotado y puede quedar pendiente sin eliminar datos protegidos. `needs_expansion` señala una representación parcial: se amplía cuando la tarea necesita un hecho o condición ausente, sin afirmar cobertura completa de la nota.
