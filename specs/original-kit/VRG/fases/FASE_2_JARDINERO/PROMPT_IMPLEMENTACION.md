# PROMPT_IMPLEMENTACION.md — Ejecutar solo fase 2

Actúa como coordinador de la limpieza aislada VRG. Lee la matriz **aprobada** de fase 1 y `FASE_2.md` + `GOAL.md`. Si no existe aprobación explícita y listado exacto de rutas a retirar, **detente** sin borrar nada.

## Orden de trabajo

1. Confirma estado, branch, worktree y copia de seguridad dentro del alcance autorizado; protege notas originales y cambios preexistentes.
2. Construye una lista de operaciones a partir de los IDs/rutas exactos aprobados; no añadir candidatos nuevos.
3. Reparte hasta 8–12+ grupos de carpetas disjuntas por agente para eliminar en paralelo.
4. Cada agente elimina o aísla solo lo autorizado y entrega un inventario compacto de modificaciones; sin ejecutar el producto.
5. Coordina las modificaciones de archivos compartidos para evitar conflictos.
6. Escribe `REPORTE_JARDINERO.md` con matriz aprobado→hecho→ruta→motivo→rollback y problemas no abordados.
7. Termina al cumplir el GOAL. La versión puede estar intencionalmente inoperativa.

## Prohibiciones

Nada de tests, cargo check, compilación, reparaciones funcionales, nuevos módulos Rust, migración de notas, alteración del repositorio original o de `~/.codex`, borrados fuera del worktree, ni expansión automática de la lista autorizada. No pegar logs en conversación.

## Manejo de errores

Si un archivo está modificado por otro agente, si una ruta no coincide con la aprobada o si los datos se ubican en fuente original: detener **solo esa operación**, no improvisar ni forzar. Documentar bloqueo; continuar tareas independientes seguras.

## Salida

Estado `FASE_2_COMPLETA_AISLADA` o `FASE_2_PARCIAL/BLOQUEADA`, cambios exactos, seguridad del material original y pendientes. No pasar automáticamente a fase 3.
