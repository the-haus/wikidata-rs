-- MySQL dump 10.19  Distrib 10.3.38-MariaDB, for debian-linux-gnu (x86_64)
--
-- Host: 10.64.0.1    Database: wikidatawiki
/*!40101 SET NAMES binary*/;
DROP TABLE IF EXISTS `redirect`;
CREATE TABLE `redirect` (
  `rd_from` int(8) unsigned NOT NULL DEFAULT 0,
  `rd_namespace` int(11) NOT NULL DEFAULT 0,
  `rd_title` varbinary(255) NOT NULL DEFAULT '',
  `rd_interwiki` varbinary(32) DEFAULT NULL,
  `rd_fragment` varbinary(255) DEFAULT NULL,
  PRIMARY KEY (`rd_from`)
) ENGINE=InnoDB DEFAULT CHARSET=binary;
/*!40000 ALTER TABLE `redirect` DISABLE KEYS */;
INSERT INTO `redirect` VALUES (9000001,0,'Q42','',''),(9000007,0,'Q55','',''),(9000008,0,'Q99999999','','');
INSERT INTO `redirect` VALUES (9000009,0,'Q10','',''),(9000010,0,'Q42','',''),(9000031,120,'P31','','');
/*!40000 ALTER TABLE `redirect` ENABLE KEYS */;
