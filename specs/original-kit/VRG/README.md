> Public release note: this directory contains selected historical specification excerpts only. Some referenced local documents are deliberately omitted for privacy; this is not the complete original kit or the current operational instructions.

# VRG — Vault RAG Graph · Spec Kit de proyecto v0.1

**Estado:** kit de instrucciones inicial, no implementado ni auditado.
**Fecha de decisión:** 2026-10-29.
**Lenguaje de implementación:** Rust.
**Sistema objetivo:** Void Linux; ejecución y almacenamiento locales como opción predeterminada.
**Origen de conocimiento:** vaults de Obsidian y componentes reales autorizados en fase 1.

## Propósito

Modernizar la memoria de Obsidian existente en un sistema local de recuperación híbrida **RAG + grafo**, sin perder la experiencia de navegación y edición Markdown de Obsidian, y exponer capacidades a Codex mediante MCP local y a ChatGPT mediante un conector compatible cuando sea técnicamente viable y autorizado.

**Advertencia importante:** Este paquete implementa un *flujo Spec Kit personalizado* en Markdown compatible conceptualmente con desarrollo guiado por especificación; no afirma haber ejecutado `specify init` ni instalado la CLI oficial. Una futura integración con la CLI no autoriza reescribir ni ampliar las cinco fases.

## Orden de lectura obligatorio

1. `AGENTS.md` y `CONSTITUCION.md` (una vez al inicio o si cambian).
2. `DECISIONES_Y_PENDIENTES.md` y el documento original de requisitos de la tarea.
3. `fases/FASE_N_*/FASE_N.md` y su `GOAL.md` y `PROMPT_IMPLEMENTACION.md` **únicamente de la fase activa**.
4. Solo las secciones del informe anterior necesarias para ejecutar la fase activa.

No cargar repetidamente todos los documentos. La documentación principal tiene prioridad sobre resúmenes RAG y memoria conversacional. Si existen contradicciones, señalar el conflicto, no inventar una resolución. Las instrucciones superiores de seguridad siempre se respetan.

## Plan de fases y bloqueos

| Fase | Documento | Inicio permitido | Resultado / condición de salida |
|---|---|---|---|
| 1: Auditoría | `fases/FASE_1_AUDITORIA/FASE_1.md` | Tras señalar rutas o detectar fuentes autorizadas accesibles | `INFORME_AUDITORIA_REAL.md` exhaustivo, sin cambios de producto |
| 2: Jardinero | `fases/FASE_2_JARDINERO/FASE_2.md` | Usuario revisó la auditoría y aprobó matriz KEEP/ADAPT/REMOVE | Limpieza en rama/worktree aislado, sin obligación de funcionar |
| 3: Construcción | `fases/FASE_3_IMPLEMENTACION/FASE_3.md` | Arquitectura concretada desde la auditoría y lista de pruebas aprobada | Módulos programados; `cargo check` y handshake autorizados |
| 4: Certificación | `fases/FASE_4_CERTIFICACION/FASE_4.md` | Implementación completa de fase 3 | Solo pruebas aprobadas y correcciones, matriz de recuperación de fallos |
| 5: Migración | `fases/FASE_5_MIGRACION/FASE_5.md` | Fase 4 certificada y migración aprobada | Índices del vault completos, originales intactos y verificaciones autorizadas |

## Qué se entrega en este paquete

- 15 documentos: `FASE_N.md`, `PROMPT_IMPLEMENTACION.md`, `GOAL.md` para cada una de cinco fases.
- `AGENTS.md`, `CONSTITUCION.md`, `PROTOCOLO_MULTIAGENTE.md` y `DECISIONES_Y_PENDIENTES.md`.
- `plantillas/INFORME_AUDITORIA_REAL_TEMPLATE.md`, `plantillas/REGISTRO_PRUEBAS_CERRADO.md`.
- Este `README.md`.

## Dos distinciones críticas

**Leer código no es ejecutar pruebas.** Fase 1 solo inspecciona repositorios, configuración y archivos existentes; los comandos de inventario de solo lectura no deben ejecutarlo ni activar hooks peligrosos.

**Migrar notas a RAG no significa convertir ni mover las notas originales.** Es indexarlas por referencia a rutas/IDs y generar embeddings, índice textual y relaciones; los archivos `.md` mantienen control editorial en Obsidian. Cualquier migración física o reescritura masiva requiere autorización explícita adicional.

## Inicio recomendado para Codex

> Lee `AGENTS.md`, `CONSTITUCION.md`, `DECISIONES_Y_PENDIENTES.md` y los tres documentos de `fases/FASE_1_AUDITORIA/`. Activa **solamente la fase 1**. Descubre de forma no destructiva las rutas autorizadas y audita exclusivamente código real accesible. No ejecutes tests, no generes código de producto y no pases a fase 2. Crea el informe exhaustivo en Markdown, separando hechos, evidencias y pendientes. Utiliza agentes paralelos con propiedad exclusiva por área. Muestra avances extremadamente breves y un informe final completo.

## Para integrar con GitHub Spec Kit oficial

La CLI oficial admite flujos personalizables y Codex. Esta carpeta es un kit independiente, utilizable sin CLI. Si se instala el producto oficial, preservar este flujo y comprobar cómo se integran sus archivos; **no** asumir que el scaffolding nativo reemplaza los documentos ya aprobados.
