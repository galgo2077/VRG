# GOAL.md — Fase 5 (alcance congelado)

**Objetivo único:** incorporar todas las notas Markdown de los vaults autorizados a un índice RAG híbrido y grafo local, preservando por completo los originales y habilitando reversión de índices.

**Prerrequisitos:** fase 4 certificada; roots/exclusiones/backups/política aprobadas; lista cerrada fase 5 aprobada antes de fase 3.

**Criterios de terminación:**

- Universo de notas autorizado inventariado, con excluidas y fallidas contabilizadas.
- Índice textual, semántico y grafo derivados construidos y publicados con journal y versión, o reporte explícito de bloqueo.
- Notas `.md`, enlaces y estructura originales preservados.
- Cobertura, recuentos y limitaciones registrados en `INFORME_MIGRACION.md`.
- **Solo** comprobaciones posmigración aprobadas, sin tests nuevos, ejecutadas al final.
- Rollback de índices disponible y documentación de recuperación ante fallos.

**STOP:** una vez concluido el universo autorizado, terminar. No extender a archivos no autorizados, no reorganizar el vault, no automatizar entrenamientos, no convertir Markdown a formato propietario, no expandir tests ni iniciar fase 6.
