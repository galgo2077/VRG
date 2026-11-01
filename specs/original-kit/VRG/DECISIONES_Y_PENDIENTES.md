# Decisiones aprobadas y pendientes de VRG

## Confirmado por el usuario

| ID | Decisión | Estado |
|---|---|---|
| D01 | Proyecto `VRG` (`Vault RAG Graph`) | DEFINIDO |
| D02 | Repositorio nuevo, independiente y vinculado a Obsidian | DEFINIDO |
| D03 | Auditoría de vaults, scripts, hooks, sincronización, MCP y Codex | DEFINIDO |
| D04 | Jardinero en rama/worktree aislado, backup, notas originales intactas | DEFINIDO |
| D05 | Lectura y escritura futura de notas automáticas con historial/reversión | DEFINIDO, detalles de política por definir |
| D06 | Grafo nativo de Obsidian + relaciones RAG complementarias | DEFINIDO |
| D07 | Codex por MCP; ChatGPT por interfaz compatible y condicionada | DEFINIDO, conexión real condicionada |
| D08 | Local/offline por defecto en Void Linux | DEFINIDO |
| D09 | Markdown, metadatos, enlaces y referencias al código | DEFINIDO |
| D10 | ~8 o más agentes paralelos cuando útiles; sin límite rígido | DEFINIDO |
| D11 | Fase 3 únicamente `cargo check` y handshake mínimo | DEFINIDO |
| D12 | Lista cerrada de pruebas derivada de auditoría y aprobada antes de implementar | DEFINIDO |
| D13 | Fallos: fail-closed, recuperación automática segura, log compacto | DEFINIDO |
| D14 | Fase 5: incorporar todas las notas al RAG como último paso | DEFINIDO, migración lógica no destructiva |

## Sin definir — no inventar

| ID | Pendiente | Resolución mínima | Momento |
|---|---|---|---|
| P01 | Rutas reales de repositorios, vaults, hooks y scripts | Inspección de ámbitos autorizados o confirmación del usuario | Inicio fase 1 |
| P02 | Inventario y clasificación del código existente | Auditoría con evidencias | Final fase 1 |
| P03 | Versiones concretas y dependencias de crates Rust | Decisión basada en auditoría y soporte real | Antes de fase 3 |
| P04 | Herramientas exactas de embeddings/índice/vector/grafo | Seleccionar tras evidencias, límites de hardware y privacidad | Antes de fase 3 |
| P05 | Herramientas/alcance exacto de MCP de ChatGPT y permisos de cuenta | Revisión de compatibilidad, sin asumir disponibilidad | Fase 3 |
| P06 | Pruebas exactas y umbrales de fase 4 y de migración fase 5 | Propuesta derivada de fase 1 y aprobación expresa | Antes de fase 3 |
| P07 | Exclusiones, vaults y notas sensibles | Inventario y política aprobada | Antes de fase 5 |
| P08 | Alcance de escrituras automáticas y recuperación de conflictos | Política derivada del vault real, con límites claros | Antes de fase 3 |
| P09 | Tamaño y conectividad del grafo real | Informe de fase 1 | Fase 1 |

## Cambios y control de alcance

Los puntos `P*` no permiten crear implementaciones especulativas. Cuando un bloqueo material aparezca, documentar el límite y continuar con trabajo independiente. Si se requiere decisión del usuario para la siguiente fase, terminar la actual con `BLOQUEADO_POR_DECISION` en lugar de imaginar aprobación.
