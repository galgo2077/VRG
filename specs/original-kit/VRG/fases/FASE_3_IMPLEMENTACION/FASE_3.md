# FASE_3.md — Programación completa de VRG en Rust

## Activación

Solo después de: auditoría real revisada por el usuario; Jardinero terminado en worktree; arquitectura/casos de uso aceptados; lista cerrada de pruebas para fases 4 y 5 **aprobada por el usuario antes de escribir módulos**. La ausencia de una decisión material bloquea únicamente el módulo afectado, no habilita invención.

## Objetivo

Construir las capacidades aprobadas de VRG en módulos Rust independientes y conectarlas sin ejecutar suites funcionales. **Primero implementar TODO el código aprobado, después una comprobación final agrupada** consistente en `cargo check` y un handshake mínimo autorizado. No probar por módulos ni entre etapas.

## Contratos/capacidades candidatas (no son hallazgos de auditoría)

- `vrg-core`: modelos, IDs de fuente, revisiones, errores tipados, estados.
- `vrg-vault`: lectura Markdown, frontmatter, aliases, wikilinks, transclusiones, rutas seguras y exclusiones.
- `vrg-graph`: enlaces explícitos Obsidian y relaciones derivadas etiquetadas por origen; backlinks.
- `vrg-text-search`: búsqueda exacta y léxica de símbolos, rutas y notas.
- `vrg-semantic`: embeddings locales, índice vectorial, pertinencia y procedencia.
- `vrg-retrieval`: combinación híbrida, presupuesto de tokens, filtros y explicabilidad.
- `vrg-indexer`: cobertura/incrementalidad, hash de fuente, versionado y planificación de shards.
- `vrg-storage`: persistencia atómica de índices derivados, commits y checkpoints.
- `vrg-history`: journal, backup, conflictos, write-guard y rollback de operaciones permitidas.
- `vrg-mcp`: herramientas MCP locales de búsqueda/lectura/resolución y escrituras autorizadas.
- `vrg-obsidian`: sincronía y mapeos de notas al grafo visible, sin cambiar el formato de notas.
- `vrg-cli`: operaciones de mantenimiento y diagnósticos compactos.
- `vrg-chatgpt-bridge`: **adaptador opcional y condicionado** a conectividad externa segura y permisos del producto, sin prometer ejecución offline.

Estas son superficies de diseño, **no** obligación de crear exactamente 13 crates: unificar o subdividir según la arquitectura aprobada para minimizar costes. Ninguna librería externa concreta se considera seleccionada hasta documentar compatibilidad, licencia y consumo local.

## Requisitos funcionales contractuales

1. Rutas de notas y enlaces originales inalterados por la creación del índice.
2. Los resultados incluyen cita a archivo/fragmento/revisión y relación (explícita vs. semántica), sin fabricar fuentes.
3. Filtrado `deny-by-default` de notas/rutas no autorizadas y exclusión de secretos/archivos de sistema.
4. Indexación incremental e idempotente con manejo de renames, tombstones y cambios concurrentes.
5. Índices derivados reconstruibles, versionados y separables de notas originales.
6. Recuperación híbrida que favorezca coincidencia exacta de rutas/código y complemente con semántica/grafo.
7. Fallos críticos no deben producir escritura parcial o exposición de datos; política fail-closed.
8. Escritura opcional de notas autorizadas solo con journal, historial, revisión esperada y rollback; sin modificación silenciosa de notas en la fase 3.
9. API pública del núcleo Rust desacoplada de MCP/chat; límites de recursos explicitados.
10. Presupuesto de salida del RAG configurable, con supresión de fragmentos duplicados y fuentes repetidas.
11. Concurrencia controlada por namespace de vault y por archivo; caches y commits coordinados.
12. Registro compacto sin contenido sensible, mensajes tipados para poder diagnosticar y reparar en fase 4.

## Paralelismo

Congelar contratos de interfaz antes de dar inicio a módulos dependientes. Repartir 8–12+ frentes independientes por crates o carpetas exclusivas. Los agentes comparten **interfaces y tipos**, no editan simultáneamente el mismo archivo. El coordinador integra cambios, sin relanzar ciclos de pruebas por integración intermedia.

## Únicas comprobaciones permitidas, AL TERMINAR TODO

- `cargo check --workspace` (o comando equivalente exacto aprobado para el workspace real). Si falla, corregir compilación y volver a ejecutar `cargo check` para confirmar; no ejecutar pruebas.
- **Handshake mínimo de comunicación**: únicamente inicialización y descubrimiento de herramientas de un servidor MCP local de VRG por cliente Codex, con identificación y respuesta de estado; ningún test de funcionalidad, escritura de notas o benchmark.
- Conector ChatGPT: diseñar e implementar interfaz compatible si el usuario y el entorno lo permiten; un handshake real solo si hay mecanismo de acceso aprobado y disponible. Si no, declarar `CHATGPT_INTEGRACION_PENDIENTE` sin fingir conexión ni exponer puertos públicos.

No añadir `cargo test`, doctests, `cargo clippy`, `cargo fmt --check`, benchmarks, fuzz, integraciones productivas, tests de grafos o embeddings en esta fase.

## Manejo de errores durante construcción

Diseñar propagación tipada de fallos y contratos fail-closed; **no ejecutar escenarios para validar comportamiento aún**. Tabla de fallos inicial a llevar a fase 4: archivo ausente, enlace ambiguo, symlink externo, formato inválido, índice corrupto, embeddings no disponibles, timeout de MCP, conflicto de revisión, falta de espacio, interrupción de commit y fuente no autorizada.

## Entregables

- Código completo de módulos aprobados en branch/worktree aislado.
- `CONTRATOS_RUST.md`: APIs públicas, propiedad de datos, versión y errores.
- `INFORME_IMPLEMENTACION.md`: módulos, decisiones, estado de compilación y handshake, límites.
- `MATRIZ_FALLOS_DISENO.md`: escenario → detección prevista → respuesta diseñada → recuperación prevista (todavía **no probada**).

## Parada

Cuando todo el código del alcance aprobado está integrado y se ejecutaron solo las dos comprobaciones autorizadas, detenerse. La fase 4 necesita un runner diferente según la lista cerrada. **No decir que VRG funciona porque compiló.**
