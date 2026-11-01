# GOAL.md — Fase 3 (alcance congelado)

**Objetivo único:** implementar por completo en Rust los módulos **aprobados** para VRG y dejar definidos contratos, políticas de error y conectores, sin probar funcionalidades.

**Prerrequisitos:** arquitectura basada en auditoría real, repositorio aislado, lista cerrada de pruebas fases 4–5 aprobada antes de la programación.

**Criterios de finalización:**

- Todos los módulos comprometidos se implementaron y están integrados.
- Un `cargo check --workspace` final da resultado conocido (si falla, no marcar compilación completa; corregir dentro del alcance).
- Handshake mínimo de inicialización/herramientas con Codex local ejecutado y documentado.
- ChatGPT registrado como conectado únicamente si fue verificable con permisos reales; en caso contrario, pendiente explícito.
- Matriz de fallos diseñados y contratos Rust documentados.
- Sin tests funcionales, benchmarks ni escrituras masivas de notas.

**STOP:** después de esas comprobaciones, no ejecutar un solo test de fase 4; no indexar todo el vault; no refactorizar sin nuevo requisito. Si una integración no disponible impide un criterio, registrar bloqueo en vez de consumir tokens buscando soluciones ilimitadas.
