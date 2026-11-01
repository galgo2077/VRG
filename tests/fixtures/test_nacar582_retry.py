"""Comprobaciones de la política sintética NACAR582."""

import unittest
import json

from nacar582_retry import (cabe_en_presupuesto, espera_segundos,
                            espera_total_segundos, planificar_reintentos,
                            crear_checkpoint, reanudar_checkpoint)


class EsperaSegundosTest(unittest.TestCase):
    def test_reintentos_hasta_el_limite(self):
        for reintentos, esperado in enumerate((17, 20, 23, 26, 29, 32, 35)):
            with self.subTest(reintentos=reintentos):
                self.assertEqual(espera_segundos(reintentos), esperado)

    def test_reintentos_extremos_se_limitan_antes_de_multiplicar(self):
        for reintentos in (7, 2**64 - 1, 2**1000):
            with self.subTest(reintentos=reintentos):
                self.assertEqual(espera_segundos(reintentos), 35)

    def test_entradas_invalidas(self):
        with self.assertRaises(ValueError):
            espera_segundos(-1)
        for valor in (1.5, "1", True, None):
            with self.subTest(valor=valor), self.assertRaises(TypeError):
                espera_segundos(valor)


class PresupuestoTest(unittest.TestCase):
    def test_cero_y_limite_exacto(self):
        for reintentos, presupuesto, esperado in ((0, 0, True), (1, 0, False),
                                                  (6, 164, False), (6, 165, True)):
            with self.subTest(reintentos=reintentos, presupuesto=presupuesto):
                self.assertEqual(cabe_en_presupuesto(reintentos, presupuesto), esperado)

    def test_valores_extremos(self):
        for reintentos in (2**64 - 1, 2**1000):
            with self.subTest(reintentos=reintentos):
                self.assertFalse(cabe_en_presupuesto(reintentos, 164))
                self.assertTrue(cabe_en_presupuesto(reintentos, 165))
                self.assertTrue(cabe_en_presupuesto(reintentos, 2**1000))

    def test_entradas_invalidas(self):
        for reintentos, presupuesto, error in ((-1, 0, ValueError), (True, 0, TypeError),
                                               (0, -1, ValueError), (0, True, TypeError),
                                               (0, 1.5, TypeError), (0, None, TypeError)):
            with self.subTest(reintentos=reintentos, presupuesto=presupuesto):
                with self.assertRaises(error):
                    cabe_en_presupuesto(reintentos, presupuesto)


class PlanificarReintentosTest(unittest.TestCase):
    def test_limites_y_reanudacion(self):
        self.assertEqual(planificar_reintentos(19, 0), [])
        self.assertEqual(planificar_reintentos(20, 0), [(1, 20)])
        self.assertEqual(planificar_reintentos(43, 1), [(2, 23)])
        self.assertEqual(planificar_reintentos(19, 1), [])
        self.assertEqual(planificar_reintentos(165, 6), [])
        self.assertEqual(planificar_reintentos(2**1000, 2**1000), [])

    def test_seguridad_y_equivalencia_al_reanudar(self):
        for presupuesto in range(201):
            completo = planificar_reintentos(presupuesto, 0)
            for ejecutados in range(7):
                with self.subTest(presupuesto=presupuesto, ejecutados=ejecutados):
                    pendiente = planificar_reintentos(presupuesto, ejecutados)
                    if espera_total_segundos(ejecutados) > presupuesto:
                        self.assertEqual(pendiente, [])
                        continue
                    self.assertEqual(pendiente, completo[ejecutados:])
                    self.assertEqual([n for n, _ in pendiente],
                                     list(range(ejecutados + 1, ejecutados + 1 + len(pendiente))))
                    self.assertLessEqual(espera_total_segundos(ejecutados) +
                                         sum(espera for _, espera in pendiente), presupuesto)
                    self.assertTrue(all(espera == espera_segundos(n) for n, espera in pendiente))
                    siguiente = ejecutados + len(pendiente) + 1
                    if siguiente <= 6:
                        self.assertGreater(espera_total_segundos(siguiente), presupuesto)

    def test_entradas_invalidas(self):
        for presupuesto, ejecutados, error in ((-1, 0, ValueError), (0, -1, ValueError),
                                               (True, 0, TypeError), (0, True, TypeError),
                                               (1.5, 0, TypeError), (0, None, TypeError)):
            with self.subTest(presupuesto=presupuesto, ejecutados=ejecutados):
                with self.assertRaises(error):
                    planificar_reintentos(presupuesto, ejecutados)


class CheckpointTest(unittest.TestCase):
    def test_continuidad_en_todos_los_prefijos_y_presupuestos(self):
        for presupuesto in range(201):
            completo = planificar_reintentos(presupuesto, 0)
            for ejecutados in range(7):
                if espera_total_segundos(ejecutados) <= presupuesto:
                    with self.subTest(presupuesto=presupuesto, ejecutados=ejecutados):
                        checkpoint = crear_checkpoint(presupuesto, ejecutados)
                        self.assertEqual(reanudar_checkpoint(checkpoint), completo[ejecutados:])
        self.assertEqual(reanudar_checkpoint(crear_checkpoint(2**1000, 6)), [])

    def test_rechaza_estados_incompatibles(self):
        valido = json.loads(crear_checkpoint(43, 1))
        cambios = ({"version": 2}, {"version": True}, {"politica": "otra"},
                   {"ejecutados": 7}, {"ejecutados": -1}, {"ejecutados": True},
                   {"presupuesto_segundos": 19}, {"presupuesto_segundos": False},
                   {"acumulado_segundos": 21}, {"acumulado_segundos": 20.0},
                   {"extra": 1})
        for cambio in cambios:
            with self.subTest(cambio=cambio), self.assertRaises(ValueError):
                reanudar_checkpoint(json.dumps(valido | cambio))
        for campo in valido:
            incompleto = valido.copy()
            del incompleto[campo]
            with self.subTest(campo=campo), self.assertRaises(ValueError):
                reanudar_checkpoint(json.dumps(incompleto))
        for ejecutados, presupuesto in ((7, 1000), (1, 19), (-1, 100)):
            with self.assertRaises(ValueError):
                crear_checkpoint(presupuesto, ejecutados)
        with self.assertRaises(TypeError):
            crear_checkpoint(100, True)

    def test_json_no_confiable(self):
        for datos in ('', '{', '[]', 'null', '{"version":1,"version":1}',
                      ' ' * 4097, '[' * 1100 + ']' * 1100):
            with self.subTest(datos=datos[:30]), self.assertRaises(ValueError):
                reanudar_checkpoint(datos)
        with self.assertRaises(TypeError):
            reanudar_checkpoint(b'{}')


class EsperaTotalSegundosTest(unittest.TestCase):
    def test_totales_hasta_el_limite(self):
        for reintentos, esperado in enumerate((0, 20, 43, 69, 98, 130, 165)):
            with self.subTest(reintentos=reintentos):
                self.assertEqual(espera_total_segundos(reintentos), esperado)

    def test_limite_y_valores_extremos(self):
        for reintentos in (7, 2**64 - 1, 2**1000):
            with self.subTest(reintentos=reintentos):
                self.assertEqual(espera_total_segundos(reintentos), 165)

    def test_entradas_invalidas(self):
        with self.assertRaises(ValueError):
            espera_total_segundos(-1)
        for valor in (1.5, "1", True, None):
            with self.subTest(valor=valor), self.assertRaises(TypeError):
                espera_total_segundos(valor)


if __name__ == "__main__":
    unittest.main()
