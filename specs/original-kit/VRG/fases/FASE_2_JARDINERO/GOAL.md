# GOAL.md — Fase 2 (alcance congelado)

**Objetivo único:** retirar del entorno aislado solo componentes de código explícitamente aprobados para eliminación, sin modificar los archivos originales de Obsidian.

**Entradas vinculantes:** informe de fase 1 + decisión explícita del usuario + lista exacta de rutas autorizadas.

**Hecho significa:**

- Aislamiento branch/worktree y protección de originales documentados.
- Elementos autorizados eliminados o bloqueos claramente identificados.
- `REPORTE_JARDINERO.md` con registro y reversibilidad.
- Componentes conservados para fase 3 identificables.

**No requerido:** que compile, que funcione, que el usuario pueda conectarse, ninguna prueba.

**STOP:** terminar inmediatamente cuando la matriz autorizada se haya procesado. No ampliar limpieza, no arreglar lo que se rompió, no ejecutar fase 3.
