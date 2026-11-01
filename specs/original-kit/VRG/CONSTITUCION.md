# Constitución de VRG — invariantes no negociables

## C-01. Autoridad documental

La especificación original de cada tarea define el producto; el kit define cómo trabajar. El informe de fase 1 describe hechos, no autoriza decisiones destructivas por sí solo. Las modificaciones a requisitos o tests requieren decisión explícita del usuario.

## C-02. Obsidian sigue siendo el cerebro visible

Los archivos Markdown `.md`, sus rutas, wikilinks, frontmatter y organización siguen accesibles y editables con Obsidian. El índice RAG y el grafo son **derivados reconstruibles**; no reemplazan ni encapsulan el contenido original.

## C-03. Privacidad y local-first

Por defecto, indexación, embeddings y recuperación locales. No enviar notas, hashes, embeddings ni rutas sensibles a proveedores externos. ChatGPT requiere integración específica y consentimiento: **local/offline absoluto y acceso remoto desde ChatGPT no pueden garantizarse simultáneamente**. Priorizar Codex local y dejar la integración ChatGPT como adaptador condicionado.

## C-04. Preservación y reversibilidad

Los originales y repositorios existentes no se limpian destructivamente. Usar un nuevo repositorio VRG, worktree/branch independiente, backups y manifiestos de trazabilidad. Ninguna rutina de limpieza puede modificar las notas originales. Escrituras futuras a notas: atómicas, versionadas, reversibles y autorizadas por políticas del vault.

## C-05. Evidencia real

No fingir que existe código, estructura de vault, APIs, rutas, interfaces ni benchmarks. Todo hallazgo de auditoría se vincula a evidencia real; lo no verificable se marca `DESCONOCIDO`/`NO_INSPECCIONADO`.

## C-06. Separación de responsabilidades

Núcleo Rust independiente de MCP, Codex y la interfaz Obsidian. Conectores adaptan a la API pública; las notas, el índice textual, el índice semántico y el grafo tienen contratos y recuperación separados. No fijar bibliotecas definitivas antes de conocer el código real.

## C-07. Recuperación con atribución

Cada resultado RAG devuelve fuente original, ruta/identificador, fragmento, revisión y naturaleza de la relación. Un resultado probabilístico nunca se presenta como requisito obligatorio. Priorizar coincidencias exactas para símbolos, rutas e instrucciones.

## C-08. Política estricta de pruebas

- Fase 1: ninguna prueba ni ejecución de producto.
- Fase 2: ninguna prueba ni compilación obligatoria.
- Fase 3: después de implementar todo, únicamente `cargo check` y handshake mínimo definido; sin `cargo test`, lint, benchmark ni suites adicionales.
- Fase 4: ejecutar exclusivamente pruebas de la **lista cerrada escrita y aprobada antes de fase 3**; no agregar pruebas unilateralmente.
- Fase 5: solo comprobaciones de migración enumeradas en esa **misma lista cerrada**; no abrir nuevas pruebas tras la aprobación.
- Si una prueba aprobada falla, corregir el componente y repetir solo la(s) prueba(s) aprobada(s) relacionada(s). No fabricar cobertura.
- Si falta aprobación, detener la etapa que exige ese permiso y registrar el bloqueo.

## C-09. Eficiencia de tokens

Leer selectivamente, usar diffs, evitar reabrir textos inmutables, no pegar logs, evitar exploración infinita, minimizar agentes redundantes. Avances intermedios de 1–3 líneas; reporte final completo y basado en evidencia. No resumir exhaustividad del informe fase 1: **el artefacto puede ser largo, el chat debe ser breve**.

## C-10. Paralelismo sin conflictos

Objetivo ~8–12 agentes activos o más cuando existan tareas independientes. Un archivo writable tiene un solo propietario a la vez, integraciones controladas, dependencias explícitas y límite por memoria/CPU. Un único ejecutor de pruebas en fase 4; sin duplicar suites por agente.

## C-11. Fail-closed y degradación explícita

Frente a incertidumbre de permisos, rutas, integridad, conflicto de revisiones, datos sensibles o escritura: bloquear la operación riesgosa y registrar razón compacta. La degradación de búsqueda semántica a textual solo puede ser **explícita**, identificada como modo degradado; nunca ocultar pérdida de capacidades.

## C-12. Goals cerrados

Cada `GOAL.md` define entradas, entregables, aceptación y condición de parada. Prohibido autoexpandir alcance, perseguir 100 % imaginario, iniciar fases siguientes, ejecutar pruebas no indicadas o crear proyectos subsidiarios.

## C-13. Seguridad de índices y claves

Excluir secretos, `.git`, tokens, backups privados y directorios fuera de raíces autorizadas; no seguir symlinks que escapen al vault. Aislar espacios de nombres, normalizar rutas de forma segura, conservar política de exclusiones y minimizar exposición vía MCP.

## C-14. Transparencia de estado

Distinguir `IMPLEMENTADO`, `COMPILA`, `CONECTADO`, `PROBADO`, `MIGRADO`, `VALIDADO` y `BLOQUEADO`. Un `cargo check` no demuestra funcionamiento; un handshake local no demuestra que ChatGPT pueda conectar; un índice generado no demuestra que la migración haya superado pruebas.
