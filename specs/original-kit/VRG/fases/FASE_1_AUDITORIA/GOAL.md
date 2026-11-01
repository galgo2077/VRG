# GOAL.md — Fase 1 (alcance congelado)

**Objetivo único:** producir un informe Markdown exhaustivo, veraz y verificable sobre el código y los vaults existentes, con matriz de conservación/reemplazo y propuesta de fases futuras.

**Incluye:** inventario real, clasificación por componente, riesgos, evidencia, dependencias, diseño candidato, acciones de Jardinero sugeridas, lista *propuesta* de pruebas mínimas y preguntas abiertas.

**Excluye:** modificar código/notas, tests, compilación, ejecución funcional, eliminación, instalación, elección definitiva de librerías, inicio de fase 2.

**Criterios de cierre:**

- Un informe íntegro `INFORME_AUDITORIA_REAL.md`, amplio pero con evidencia para afirmaciones concretas.
- Todas las áreas autorizadas y accesibles inventariadas o etiquetadas `NO_INSPECCIONADO`.
- Clasificación `MANTENER/ADAPTAR/REEMPLAZAR/ELIMINAR/DESCONOCIDO` aplicada y justificada.
- Identificados los datos que deberán preservarse y las decisiones que exigen aprobación.
- Propuesta de pruebas sin autorizar ni ejecutar.
- Entrega final que explica claramente cualquier cobertura pendiente.

**STOP:** al cumplir estos criterios, no mejorar ni extender el informe por estética, no reabrir hallazgos sin nueva evidencia, no avanzar a fase 2. Si faltan rutas autorizadas, bloquear específicamente lo inaccesible y terminar con estado parcial.
